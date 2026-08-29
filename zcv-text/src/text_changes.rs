//! 文本变化管线：为每个消费者独立累积从上次消费版本到当前版本的组合 Patch。
//!
//! 宿主事件只负责唤醒消费者；消费者读取当前 Snapshot，并消费自己的`TextChangeBatch`。
//! 不同订阅者互不争抢，也不依赖 Buffer 上的全局待消费队列。

use std::mem;
use std::ops::Range;
use std::sync::{Arc, Mutex, Weak};

use crate::{
    transaction::Delta,
    types::{BufferVersion, ByteOffset, TextRange},
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
        let mut removed = 0usize;
        let mut inserted = 0usize;
        let mut edits = Vec::with_capacity(delta.edits().len());

        for edit in delta.edits() {
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

    pub(crate) fn from_edits(edits: Vec<PatchEdit>) -> Self {
        Self { edits }
    }
}

/// 一个订阅者从上次消费到当前版本积累的组合文本变化。
#[derive(Clone, Debug, Default)]
pub struct TextChangeBatch {
    patch: TextPatch,
    old_version: Option<BufferVersion>,
    new_version: Option<BufferVersion>,
    reset: bool,
}

impl TextChangeBatch {
    pub fn patch(&self) -> &TextPatch {
        &self.patch
    }

    pub fn old_version(&self) -> Option<BufferVersion> {
        self.old_version
    }

    pub fn new_version(&self) -> Option<BufferVersion> {
        self.new_version
    }

    pub fn requires_reset(&self) -> bool {
        self.reset
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
mod tests {
    use crate::{Buffer, BufferConfig, Edit, TransactionMetadata};

    use super::*;

    fn patch(edits: &[(Range<usize>, Range<usize>)]) -> TextPatch {
        TextPatch {
            edits: edits
                .iter()
                .cloned()
                .map(|(old, new)| PatchEdit::new(old, new))
                .collect(),
        }
    }

    #[test]
    fn composing_disjoint_and_overlapping_patches_preserves_outer_coordinates() {
        let first = patch(&[(1..3, 1..4)]);
        let disjoint = patch(&[(5..9, 5..7)]);
        assert_eq!(
            first.compose(&disjoint),
            patch(&[(1..3, 1..4), (4..8, 5..7)])
        );

        let overlapping = patch(&[(3..5, 3..6)]);
        assert_eq!(first.compose(&overlapping), patch(&[(1..4, 1..6)]));
    }

    #[test]
    fn subscriptions_are_independent_and_compose_continuous_updates() {
        let mut buffer =
            Buffer::from_text("abc".to_owned(), BufferConfig::default()).expect("应创建 Buffer");
        let first = buffer.subscribe();
        let second = buffer.subscribe();
        let initial_version = buffer.version();

        for replacement in ["x", "yz"] {
            buffer
                .edit(
                    [Edit::insert(buffer.len_bytes(), replacement).expect("插入应有效")],
                    TransactionMetadata::default(),
                )
                .expect("事务应成功");
        }

        let first_batch = first.consume();
        assert_eq!(first_batch.old_version(), Some(initial_version));
        assert_eq!(first_batch.new_version(), Some(buffer.version()));
        assert!(!first_batch.patch().is_empty());
        assert!(first.consume().is_empty());

        let second_batch = second.consume();
        assert_eq!(second_batch.patch(), first_batch.patch());
    }

    #[test]
    fn composition_collapses_edits_to_inserted_and_original_text_into_one_outer_edit() {
        let insert_before_original = patch(&[(0..0, 0..1)]);
        let delete_original = patch(&[(1..2, 1..1)]);

        assert_eq!(
            insert_before_original.compose(&delete_original),
            patch(&[(0..1, 0..1)])
        );
    }
}
