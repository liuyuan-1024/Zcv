//! 文本变化管线：为每个消费者独立累积从上次消费版本到当前版本的组合 Patch。
//!
//! 宿主事件只负责唤醒消费者；消费者读取当前 Snapshot，并消费自己的`TextChangeBatch`。
//! 不同订阅者互不争抢，也不依赖 Buffer 上的全局待消费队列。

use std::mem;
use std::ops::Range;
use std::sync::{Arc, Mutex, Weak};

use crate::{
    position_map::PositionMap,
    transaction::{Delta, DeltaEvent},
    types::{BufferVersion, ByteOffset, TextRange, TransactionId},
};

/// 一段文本变化在旧、新坐标空间中的覆盖范围。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchEdit {
    old: TextRange,
    new: TextRange,
}

impl PatchEdit {
    pub(crate) fn new(old: Range<usize>, new: Range<usize>) -> Self {
        Self {
            old: text_range(old),
            new: text_range(new),
        }
    }

    pub fn old_range(&self) -> TextRange {
        self.old
    }

    pub fn new_range(&self) -> TextRange {
        self.new
    }
}

/// 从某个已消费版本到当前版本的净文本变化。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextPatch {
    edits: Vec<PatchEdit>,
}

impl TextPatch {
    pub fn edits(&self) -> &[PatchEdit] {
        &self.edits
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    pub(crate) fn compose(&self, next: &Self) -> Self {
        let old = self.edits.iter().map(RawPatchEdit::from).collect();
        let next = next.edits.iter().map(RawPatchEdit::from).collect();
        Self {
            edits: compose_raw(old, next)
                .into_iter()
                .map(PatchEdit::from)
                .collect(),
        }
    }

    pub(crate) fn from_delta(delta: &Delta) -> Self {
        Self::from_edit_list(delta.edits())
    }

    /// 从一次事务的向前编辑构造净变化（坐标以旧文本为基准）。
    pub(crate) fn from_edit_list(source: &[crate::Edit]) -> Self {
        let mut removed = 0usize;
        let mut inserted = 0usize;
        let mut edits = Vec::with_capacity(source.len());

        for edit in source {
            let old = edit.range();
            let new_start = old
                .start()
                .get()
                .checked_sub(removed)
                .and_then(|offset| offset.checked_add(inserted))
                .expect("已验证事务的累计字节位移不应溢出");
            let new_end = new_start
                .checked_add(edit.replacement().len())
                .expect("已验证事务的新范围不应溢出");
            edits.push(PatchEdit::new(
                old.start().get()..old.end().get(),
                new_start..new_end,
            ));
            removed = removed
                .checked_add(old.len())
                .expect("已验证事务的删除字节数不应溢出");
            inserted = inserted
                .checked_add(edit.replacement().len())
                .expect("已验证事务的插入字节数不应溢出");
        }

        Self { edits }
    }

    /// 只保留旧坐标与 `range` 相交的编辑；新坐标保持组合后的绝对值。
    pub(crate) fn filtered_to_old_range(&self, range: TextRange) -> Self {
        Self {
            edits: self
                .edits
                .iter()
                .filter(|edit| {
                    let old = edit.old;
                    if old.is_empty() {
                        range.start() <= old.start() && old.start() <= range.end()
                    } else {
                        old.start() < range.end() && range.start() < old.end()
                    }
                })
                .cloned()
                .collect(),
        }
    }

    pub(crate) fn from_edits(edits: Vec<PatchEdit>) -> Self {
        Self { edits }
    }

    /// 把所有编辑的旧/新坐标整体平移；用于把某个源片段的变化换算到组合坐标。
    pub fn shifted_by(&self, shift: usize) -> Self {
        Self {
            edits: self
                .edits
                .iter()
                .map(|edit| PatchEdit {
                    old: shift_range(edit.old, shift),
                    new: shift_range(edit.new, shift),
                })
                .collect(),
        }
    }
}

fn shift_range(range: TextRange, shift: usize) -> TextRange {
    TextRange::new(
        ByteOffset::new(range.start().get() + shift),
        ByteOffset::new(range.end().get() + shift),
    )
    .expect("整体平移后的文本范围必须有序")
}

/// 一个订阅者从上次消费到当前版本积累的组合文本变化。
#[derive(Clone, Debug, Default)]
pub struct TextChangeBatch {
    patch: TextPatch,
    old_version: Option<BufferVersion>,
    new_version: Option<BufferVersion>,
    transaction_id: Option<TransactionId>,
    reset: bool,
}

impl TextChangeBatch {
    /// 为不拥有物化 Buffer 的派生文本投影创建一次整体拓扑更新。
    ///
    /// 这类投影仍然拥有独立版本与订阅边界，但没有单一 Rope 编辑可用于构造精确 patch。
    /// 消费者必须保留源锚点，并按当前快照重建派生坐标。
    pub fn reset(old_version: BufferVersion, new_version: BufferVersion) -> Self {
        Self {
            patch: TextPatch::default(),
            old_version: Some(old_version),
            new_version: Some(new_version),
            transaction_id: None,
            reset: true,
        }
    }

    /// 从一次已提交的文本事件创建显示消费者使用的单事件批次。
    ///
    /// Buffer 事件是唯一的文本变更事实；订阅只是把多个事件组合成消费者自己的批次。
    /// 需要同步显示层的直接调用方可以复用同一事件，不必再创建第二个源订阅来猜测变更范围。
    pub fn from_event(event: &DeltaEvent) -> Self {
        Self {
            patch: TextPatch::from_delta(event.delta()),
            old_version: Some(event.old_version()),
            new_version: Some(event.new_version()),
            transaction_id: Some(event.transaction_id()),
            reset: event.requires_reset(),
        }
    }

    /// 从一个源变更在输出坐标中投影出的编辑创建增量批次。
    ///
    /// 组合文档没有单一可变 Rope；
    /// 多个 excerpt 可能同时显示同一个源的不同区间，因此一次源事务可以对应多个 output 编辑。
    /// 由组合文档负责计算坐标，文本内核只负责携带版本和编辑列表。
    pub fn from_edits(
        old_version: BufferVersion,
        new_version: BufferVersion,
        edits: Vec<(TextRange, TextRange)>,
    ) -> Self {
        Self {
            patch: TextPatch::from_edits(
                edits
                    .into_iter()
                    .map(|(old, new)| PatchEdit { old, new })
                    .collect(),
            ),
            old_version: Some(old_version),
            new_version: Some(new_version),
            transaction_id: None,
            reset: false,
        }
    }

    /// 从组合后的净变化构造批次；`reset` 表示区间内发生过整体基线替换。
    pub(crate) fn from_patch(
        old_version: BufferVersion,
        new_version: BufferVersion,
        patch: TextPatch,
        reset: bool,
    ) -> Self {
        Self {
            patch,
            old_version: Some(old_version),
            new_version: Some(new_version),
            transaction_id: None,
            reset,
        }
    }

    /// 只保留旧坐标与 `range` 相交的编辑；版本区间与事务身份保持。
    pub(crate) fn filtered_to_old_range(&self, range: TextRange) -> Self {
        Self {
            patch: self.patch.filtered_to_old_range(range),
            ..self.clone()
        }
    }

    /// 用源批次的坐标映射结果创建投影批次，并保留源事务身份。
    ///
    /// 投影层可以改变编辑范围和版本空间，但不能丢失这次变化来自哪个源事务；
    /// Editor、LanguageBuffer 与 MultiBuffer 因而仍能关联到同一提交事实。
    pub fn projected_from(&self, edits: Vec<(TextRange, TextRange)>) -> Self {
        Self {
            patch: TextPatch::from_edits(
                edits
                    .into_iter()
                    .map(|(old, new)| PatchEdit { old, new })
                    .collect(),
            ),
            old_version: self.old_version,
            new_version: self.new_version,
            transaction_id: self.transaction_id,
            reset: self.reset,
        }
    }

    /// 把所有编辑坐标整体平移；用于把源片段变化换算到组合坐标。
    pub fn shifted_by(&self, shift: usize) -> Self {
        Self {
            patch: self.patch.shifted_by(shift),
            ..self.clone()
        }
    }

    /// 用新的版本区间重新发布本批次；保持增量语义。
    pub fn rebased_to(&self, old_version: BufferVersion, new_version: BufferVersion) -> Self {
        Self {
            old_version: Some(old_version),
            new_version: Some(new_version),
            ..self.clone()
        }
    }

    pub fn patch(&self) -> &TextPatch {
        &self.patch
    }

    pub fn old_version(&self) -> Option<BufferVersion> {
        self.old_version
    }

    pub fn new_version(&self) -> Option<BufferVersion> {
        self.new_version
    }

    /// 产生本批变更的底层事务身份；组合多个事务时为 None。
    pub fn transaction_id(&self) -> Option<TransactionId> {
        self.transaction_id
    }

    pub fn requires_reset(&self) -> bool {
        self.reset
    }

    /// 返回本批次从旧版本到新版本的坐标映射。
    ///
    /// 位置型派生状态必须从订阅批次取得映射，不能各自重新解释文本变更。
    /// 调用方仍需先处理 `requires_reset`，因为整体替换不保留位置跟随语义。
    pub fn position_map(&self) -> PositionMap {
        PositionMap::from_text_patch(&self.patch)
    }

    pub fn is_empty(&self) -> bool {
        self.old_version.is_none()
    }
}

#[derive(Debug)]
struct SubscriptionState {
    current_version: BufferVersion,
    pending: TextChangeBatch,
}

/// 单个消费者拥有的独立文本变化订阅。
pub struct TextSubscription(Arc<Mutex<SubscriptionState>>);

impl TextSubscription {
    pub fn consume(&self) -> TextChangeBatch {
        let mut state = self.0.lock().expect("文本变化订阅锁不应在持锁期间 panic");
        mem::take(&mut state.pending)
    }
}

#[derive(Default)]
pub(crate) struct TextChangeTopic(Mutex<Vec<Weak<Mutex<SubscriptionState>>>>);

impl std::fmt::Debug for TextChangeTopic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TextChangeTopic")
            .finish_non_exhaustive()
    }
}

impl TextChangeTopic {
    pub(crate) fn subscribe(&self, version: BufferVersion) -> TextSubscription {
        let subscription = TextSubscription(Arc::new(Mutex::new(SubscriptionState {
            current_version: version,
            pending: TextChangeBatch::default(),
        })));
        self.0
            .lock()
            .expect("文本变化主题锁不应在持锁期间 panic")
            .push(Arc::downgrade(&subscription.0));
        subscription
    }

    pub(crate) fn publish(
        &self,
        old_version: BufferVersion,
        new_version: BufferVersion,
        patch: TextPatch,
        reset: bool,
        transaction_id: Option<TransactionId>,
    ) {
        let mut subscriptions = self.0.lock().expect("文本变化主题锁不应在持锁期间 panic");
        subscriptions.retain(|subscription| {
            let Some(subscription) = subscription.upgrade() else {
                return false;
            };
            let mut state = subscription
                .lock()
                .expect("文本变化订阅锁不应在持锁期间 panic");

            if state.pending.old_version.is_none() {
                state.pending.old_version = Some(old_version);
            }
            if state.current_version != old_version {
                state.pending.reset = true;
            }
            state.pending.patch = state.pending.patch.compose(&patch);
            state.pending.new_version = Some(new_version);
            state.pending.transaction_id = match state.pending.transaction_id {
                None if state.pending.old_version == Some(old_version) => transaction_id,
                Some(existing) if Some(existing) == transaction_id => Some(existing),
                _ => None,
            };
            state.pending.reset |= reset;
            state.current_version = new_version;
            true
        });
    }
}

#[derive(Clone, Debug)]
struct RawPatchEdit {
    old: Range<usize>,
    new: Range<usize>,
}

impl From<&PatchEdit> for RawPatchEdit {
    fn from(edit: &PatchEdit) -> Self {
        Self {
            old: edit.old.start().get()..edit.old.end().get(),
            new: edit.new.start().get()..edit.new.end().get(),
        }
    }
}

impl From<RawPatchEdit> for PatchEdit {
    fn from(edit: RawPatchEdit) -> Self {
        Self::new(edit.old, edit.new)
    }
}

impl RawPatchEdit {
    fn old_len(&self) -> usize {
        self.old.end - self.old.start
    }

    fn new_len(&self) -> usize {
        self.new.end - self.new.start
    }
}

fn compose_raw(old: Vec<RawPatchEdit>, next: Vec<RawPatchEdit>) -> Vec<RawPatchEdit> {
    let mut old = old.into_iter().peekable();
    let mut next = next.into_iter().peekable();
    let mut composed = Vec::new();
    let mut old_position = 0usize;
    let mut new_position = 0usize;

    loop {
        let old_edit = old.peek_mut();
        let next_edit = next.peek_mut();

        if let Some(edit) = old_edit.as_ref()
            && next_edit
                .as_ref()
                .is_none_or(|next| edit.new.end < next.old.start)
        {
            let unchanged = edit.old.start - old_position;
            old_position += unchanged;
            new_position += unchanged;
            push_raw(
                &mut composed,
                RawPatchEdit {
                    old: old_position..old_position + edit.old_len(),
                    new: new_position..new_position + edit.new_len(),
                },
            );
            old_position += edit.old_len();
            new_position += edit.new_len();
            old.next();
            continue;
        }

        if let Some(edit) = next_edit.as_ref()
            && old_edit
                .as_ref()
                .is_none_or(|old| edit.old.end < old.new.start)
        {
            let unchanged = edit.new.start - new_position;
            old_position += unchanged;
            new_position += unchanged;
            push_raw(
                &mut composed,
                RawPatchEdit {
                    old: old_position..old_position + edit.old_len(),
                    new: new_position..new_position + edit.new_len(),
                },
            );
            old_position += edit.old_len();
            new_position += edit.new_len();
            next.next();
            continue;
        }

        let Some((old_edit, next_edit)) = old_edit.zip(next_edit) else {
            break;
        };

        if old_edit.new.start < next_edit.old.start {
            let unchanged = old_edit.old.start - old_position;
            old_position += unchanged;
            new_position += unchanged;
            let overlap_offset = next_edit.old.start - old_edit.new.start;
            let old_end = (old_position + overlap_offset).min(old_edit.old.end);
            let new_end = new_position + overlap_offset;
            push_raw(
                &mut composed,
                RawPatchEdit {
                    old: old_position..old_end,
                    new: new_position..new_end,
                },
            );
            old_edit.old.start = old_end;
            old_edit.new.start += overlap_offset;
            old_position = old_end;
            new_position = new_end;
        } else {
            let unchanged = next_edit.new.start - new_position;
            old_position += unchanged;
            new_position += unchanged;
            let overlap_offset = old_edit.new.start - next_edit.old.start;
            let old_end = old_position + overlap_offset;
            let new_end = (new_position + overlap_offset).min(next_edit.new.end);
            push_raw(
                &mut composed,
                RawPatchEdit {
                    old: old_position..old_end,
                    new: new_position..new_end,
                },
            );
            next_edit.old.start += overlap_offset;
            next_edit.new.start = new_end;
            old_position = old_end;
            new_position = new_end;
        }

        if old_edit.new.end > next_edit.old.end {
            let old_end = old_position + old_edit.old_len().min(next_edit.old_len());
            let new_end = new_position + next_edit.new_len();
            push_raw(
                &mut composed,
                RawPatchEdit {
                    old: old_position..old_end,
                    new: new_position..new_end,
                },
            );
            old_edit.old.start = old_end;
            old_edit.new.start = next_edit.old.end;
            old_position = old_end;
            new_position = new_end;
            next.next();
        } else {
            let old_end = old_position + old_edit.old_len();
            let new_end = new_position + old_edit.new_len().min(next_edit.new_len());
            push_raw(
                &mut composed,
                RawPatchEdit {
                    old: old_position..old_end,
                    new: new_position..new_end,
                },
            );
            next_edit.old.start = old_edit.new.end;
            next_edit.new.start = new_end;
            old_position = old_end;
            new_position = new_end;
            old.next();
        }
    }

    composed
}

fn push_raw(edits: &mut Vec<RawPatchEdit>, edit: RawPatchEdit) {
    if edit.old.is_empty() && edit.new.is_empty() {
        return;
    }
    if let Some(last) = edits.last_mut()
        && last.old.end >= edit.old.start
    {
        last.old.end = edit.old.end;
        last.new.end = edit.new.end;
    } else {
        edits.push(edit);
    }
}

fn text_range(range: Range<usize>) -> TextRange {
    TextRange::new(ByteOffset::new(range.start), ByteOffset::new(range.end))
        .expect("Patch 合成必须保持范围有序")
}

#[cfg(test)]
#[path = "test/text_changes_tests.rs"]
mod tests;
