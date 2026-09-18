//! 行内提示（inlay）显示层：buffer 之上、fold 之下的文本注入。
//!
//! InlayMap 是真正的增量层：它维护 `SumTree<Transform>`（Isomorphic / Inlay）， `sync(buffer_snapshot, buffer_edits)` 消费下层文本编辑并发布 `InlayEdit`，上层（FoldMap）只消费 `InlayEdit`，不再读取文本层的 PositionMap。
//!
//! `Inlay.position` 是 buffer 字节偏移，随同一批 InlayEdit 用 `remap_offset` 推进；上层不再做 inlay 坐标换算。

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use sum_tree::{Bias, ContextLessSummary, Dimension, Dimensions, Item, SumTree};
use zcv_multi_buffer::{MBTextSummary, MultiBufferOffset, MultiBufferRange, MultiBufferSnapshot};
use zcv_text::Line;

use super::chunk::InlayInfo;
use super::edit::{ProjectionEdit, remap_offset};

/// Inlay 坐标空间中的一段变化。
pub(crate) type InlayEdit = ProjectionEdit<InlayOffset>;

/// Inlay 输出字节偏移。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct InlayOffset(pub(crate) MultiBufferOffset);

impl InlayOffset {
    pub(crate) const ZERO: Self = Self(MultiBufferOffset::new(0));

    pub(crate) const fn new(value: usize) -> Self {
        Self(MultiBufferOffset::new(value))
    }

    pub(crate) const fn get(self) -> usize {
        self.0.get()
    }
}

impl std::ops::Add<usize> for InlayOffset {
    type Output = Self;
    fn add(self, rhs: usize) -> Self {
        Self(MultiBufferOffset::new(self.0.get() + rhs))
    }
}

impl std::ops::Sub for InlayOffset {
    type Output = usize;
    fn sub(self, rhs: Self) -> usize {
        self.0.get() - rhs.0.get()
    }
}

impl std::ops::AddAssign<usize> for InlayOffset {
    fn add_assign(&mut self, rhs: usize) {
        self.0 = MultiBufferOffset::new(self.0.get() + rhs);
    }
}

/// 行内提示：锚定 buffer 位置（插入在其后）+ 内容文本。
///
/// 本层只负责显示投影，不绑定数据来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Inlay {
    /// 行内提示锚定的 buffer 字节位置（插入在其后）；文本编辑时按 InlayEdit 推进。
    pub(crate) position: MultiBufferOffset,
    pub(crate) text: String,
}

#[derive(Debug, Clone)]
enum Transform {
    Isomorphic(MBTextSummary),
    Inlay(Inlay),
}

impl Item for Transform {
    type Summary = TransformSummary;

    fn summary(&self, (): ()) -> Self::Summary {
        match self {
            Transform::Isomorphic(summary) => TransformSummary {
                input: *summary,
                output: *summary,
            },
            Transform::Inlay(inlay) => TransformSummary {
                input: MBTextSummary::default(),
                output: text_summary(&inlay.text),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TransformSummary {
    input: MBTextSummary,
    output: MBTextSummary,
}

impl ContextLessSummary for TransformSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, other: &Self) {
        self.input += other.input;
        self.output += other.output;
    }
}

/// 文本的字节 / 字符 / UTF-16 / 换行摘要。
pub(super) fn text_summary(text: &str) -> MBTextSummary {
    MBTextSummary {
        len: text.len(),
        chars: text.chars().count(),
        len_utf16: text.chars().map(char::len_utf16).sum(),
        lines: text.bytes().filter(|byte| *byte == b'\n').count(),
    }
}

impl<'a> Dimension<'a, TransformSummary> for MultiBufferOffset {
    fn zero((): ()) -> Self {
        MultiBufferOffset::new(0)
    }

    fn add_summary(&mut self, summary: &'a TransformSummary, (): ()) {
        *self = MultiBufferOffset::new(self.get() + summary.input.len);
    }
}

impl<'a> Dimension<'a, TransformSummary> for InlayOffset {
    fn zero((): ()) -> Self {
        InlayOffset::ZERO
    }

    fn add_summary(&mut self, summary: &'a TransformSummary, (): ()) {
        *self += summary.output.len;
    }
}

/// 按 position 排序、投影到显示流的行内提示。
#[derive(Debug, Clone)]
pub(crate) struct InlaySnapshot {
    buffer: MultiBufferSnapshot,
    transforms: SumTree<Transform>,
    /// 每个投影行的注入段表（派生缓存，可由 transforms 重建）。
    line_inlays: Arc<BTreeMap<Line, Arc<[InlayInfo]>>>,
    version: u64,
}

impl InlaySnapshot {
    pub(super) fn buffer_snapshot(&self) -> &MultiBufferSnapshot {
        &self.buffer
    }

    /// 注入配置版本（inlay 变化信号；文本编辑不递增）。
    pub(super) const fn version(&self) -> u64 {
        self.version
    }

    /// 投影行总数（inlay 不占行数）。
    pub(super) fn line_count(&self) -> usize {
        self.buffer.line_count()
    }

    /// 流行号 → 来源行（inlay 不产生合成行，恒等）。
    pub(super) fn source(&self, line: Line) -> Option<Line> {
        (line.get() < self.buffer.line_count()).then_some(line)
    }

    pub(super) fn line_byte_range(&self, line: Line) -> Option<Range<MultiBufferOffset>> {
        let start = self.buffer.line_start_byte(line).ok()?;
        let end = if line.get() + 1 < self.buffer.line_count() {
            self.buffer
                .line_start_byte(Line::new(line.get() + 1))
                .ok()?
        } else {
            self.buffer.len_bytes()
        };
        Some(start..end)
    }

    /// 行内容的源字节范围，不含行尾换行。
    pub(crate) fn line_content_byte_range(&self, line: Line) -> Option<Range<MultiBufferOffset>> {
        let range = self.line_byte_range(line)?;
        let mut end = range.end;
        while end > range.start {
            let last = MultiBufferOffset::new(end.get() - 1);
            let is_line_break = self
                .buffer_snapshot()
                .bytes_in_range(last..end)
                .next()
                .and_then(|chunk| chunk.text.as_bytes().first().copied())
                .is_some_and(|byte| byte == b'\n' || byte == b'\r');
            if !is_line_break {
                break;
            }
            end = last;
        }
        Some(range.start..end)
    }

    /// 行的注入段信息（供渲染合成：anchor 为行内原始偏移，projected 为投影偏移）。
    pub(crate) fn line_inlays(&self, line: Line) -> &[InlayInfo] {
        self.line_inlays
            .get(&line)
            .map_or(&[], |inlays| inlays.as_ref())
    }

    /// 投影行文本（含行内提示注入）；调用方只在需要整行读取时使用（显示热路径不依赖它）。
    pub(super) fn line_text(&self, line: Line) -> Option<Cow<'_, str>> {
        let range = self.line_byte_range(line)?;
        let text = if range.is_empty() {
            Cow::Borrowed("")
        } else {
            let mut chunks = self.buffer.bytes_in_range(range);
            let first = chunks.next()?;
            if let Some(second) = chunks.next() {
                let mut text = String::from(first.text);
                text.push_str(second.text);
                text.extend(chunks.map(|chunk| chunk.text));
                Cow::Owned(text)
            } else {
                Cow::Borrowed(first.text)
            }
        };
        let inlays = self.line_inlays(line);
        if inlays.is_empty() {
            return Some(text);
        }
        let mut output = String::with_capacity(
            text.len() + inlays.iter().map(|inlay| inlay.text.len()).sum::<usize>(),
        );
        let mut cursor = 0;
        for inlay in inlays {
            output.push_str(&text[cursor..inlay.anchor]);
            output.push_str(&inlay.text);
            cursor = inlay.anchor;
        }
        output.push_str(&text[cursor..]);
        Some(Cow::Owned(output))
    }

    /// 投影行的字节长度，不拼接源文本与 inlay 文本。
    pub(crate) fn projected_line_len(&self, line: Line) -> Option<usize> {
        let range = self.line_byte_range(line)?;
        Some(
            range.end.get() - range.start.get()
                + self
                    .line_inlays(line)
                    .iter()
                    .map(|inlay| inlay.text.len())
                    .sum::<usize>(),
        )
    }

    /// 投影行内容（不含换行）的字节长度和字符数。
    pub(crate) fn projected_line_content_metrics(&self, line: Line) -> Option<(usize, usize)> {
        let content = self.line_content_byte_range(line)?;
        let content_len = content.end.get() - content.start.get();
        let mut byte = content.start.get();
        let mut chars = 0;
        while byte < content.end.get() {
            let (chunk, chunk_start) = self
                .buffer_snapshot()
                .chunk_at_byte(MultiBufferOffset::new(byte))
                .ok()?;
            let start = byte - chunk_start.get();
            let end = (content.end.get() - chunk_start.get()).min(chunk.len());
            chars += chunk[start..end].chars().count();
            byte = chunk_start.get() + end;
        }
        let inlays = self.line_inlays(line);
        Some((
            content_len + inlays.iter().map(|inlay| inlay.text.len()).sum::<usize>(),
            chars
                + inlays
                    .iter()
                    .map(|inlay| inlay.text.chars().count())
                    .sum::<usize>(),
        ))
    }

    /// 行内原始字节偏移 → 投影偏移。
    ///
    /// 面向“字符起点”语义：锚定偏移处的字符在注入文本之后，计入此前注入长度。
    pub(super) fn to_projected_offset(&self, line: Line, byte: usize) -> usize {
        byte + self
            .line_inlays(line)
            .iter()
            .take_while(|inlay| inlay.anchor <= byte)
            .map(|inlay| inlay.text.len())
            .sum::<usize>()
    }

    /// 投影偏移 → 行内原始字节偏移；落在注入段内时吸附到锚定之后（不可逆，Left bias）。
    pub(super) fn to_original_offset(&self, line: Line, projected: usize) -> usize {
        let inlays = self.line_inlays(line);
        for inlay in inlays {
            if projected >= inlay.projected && projected < inlay.projected + inlay.text.len() {
                return inlay.anchor;
            }
        }
        projected
            - inlays
                .iter()
                .take_while(|inlay| inlay.projected + inlay.text.len() <= projected)
                .map(|inlay| inlay.text.len())
                .sum::<usize>()
    }

    /// inlay 输出偏移 → buffer 偏移。
    pub(crate) fn to_buffer_offset(&self, offset: InlayOffset) -> MultiBufferOffset {
        let (start, _, item) = self
            .transforms
            .find::<Dimensions<InlayOffset, MultiBufferOffset>, _>((), &offset, Bias::Right);
        let overshoot = offset.get().saturating_sub(start.0.get());
        match item {
            Some(Transform::Isomorphic(_)) => MultiBufferOffset::new(start.1.get() + overshoot),
            Some(Transform::Inlay(_)) => start.1,
            None => self.buffer_snapshot().len_bytes(),
        }
    }
}

fn line_inlays_from_inlays(
    buffer: &MultiBufferSnapshot,
    inlays: &[Inlay],
) -> Arc<BTreeMap<Line, Arc<[InlayInfo]>>> {
    if inlays.is_empty() {
        return Arc::new(BTreeMap::new());
    }
    let mut resolved: Vec<(MultiBufferOffset, &str)> = inlays
        .iter()
        .map(|inlay| (inlay.position, inlay.text.as_str()))
        .collect();
    resolved.sort_by_key(|(offset, _)| *offset);
    let mut line_infos: BTreeMap<Line, Vec<InlayInfo>> = BTreeMap::new();
    for (offset, text) in resolved {
        let Ok(position) = buffer.byte_to_position(offset) else {
            continue;
        };
        let line = position.line();
        let Some(line_start) = buffer.line_start_byte(line).ok() else {
            continue;
        };
        let anchor = offset.get() - line_start.get();
        let entry = line_infos.entry(line).or_default();
        let projected = anchor + entry.iter().map(|inlay| inlay.text.len()).sum::<usize>();
        entry.push(InlayInfo {
            anchor,
            projected,
            text: Arc::from(text),
        });
    }
    Arc::new(
        line_infos
            .into_iter()
            .map(|(line, infos)| (line, Arc::from(infos)))
            .collect(),
    )
}

/// Inlay 投影写入口：拥有 inlay 列表与变换树。
#[derive(Debug, Clone)]
pub(super) struct InlayMap {
    snapshot: InlaySnapshot,
    inlays: Vec<Inlay>,
}

impl InlayMap {
    pub(super) fn new(buffer: MultiBufferSnapshot) -> (Self, InlaySnapshot) {
        let transforms = SumTree::from_item(Transform::Isomorphic(text_summary_for(&buffer)), ());
        let snapshot = InlaySnapshot {
            buffer,
            transforms,
            line_inlays: Arc::new(BTreeMap::new()),
            version: 0,
        };
        (
            Self {
                snapshot: snapshot.clone(),
                inlays: Vec::new(),
            },
            snapshot,
        )
    }

    pub(super) fn snapshot(&self) -> &InlaySnapshot {
        &self.snapshot
    }

    /// 消费下层文本编辑与当前 inlay 配置，推进 inlay 变换树并发布 InlayEdit。
    ///
    /// inlay 配置变化（低频）与文本编辑（高频）走同一入口：配置变化时按当前快照整体重建变换树，
    /// 文本编辑时按编辑局部拼接，并把 inlay 位置沿同一批编辑推进。
    pub(super) fn sync(
        &mut self,
        buffer_snapshot: MultiBufferSnapshot,
        mut buffer_edits: Vec<ProjectionEdit<MultiBufferOffset>>,
        inlays: Vec<Inlay>,
    ) -> (InlaySnapshot, Vec<InlayEdit>) {
        let inlay_changed = self.inlays != inlays;
        self.inlays = inlays;
        if inlay_changed {
            // inlay 配置变化：整体重建，覆盖整段文本。
            buffer_edits = vec![ProjectionEdit::new(
                MultiBufferOffset::new(0)..buffer_snapshot.len_bytes(),
                MultiBufferOffset::new(0)..buffer_snapshot.len_bytes(),
            )];
        } else {
            buffer_edits.sort_by_key(|edit| edit.old.start);
            // 先把 inlay 位置沿同一批编辑推进到新坐标空间，再重建变换树。
            for inlay in &mut self.inlays {
                if let Some(mapped) = remap_offset(inlay.position, &buffer_edits) {
                    inlay.position = mapped;
                }
            }
        }
        if buffer_edits.is_empty() {
            let snapshot = &mut self.snapshot;
            snapshot.buffer = buffer_snapshot;
            snapshot.line_inlays = line_inlays_from_inlays(&snapshot.buffer, &self.inlays);
            return (snapshot.clone(), Vec::new());
        }

        self.sort_inlays(&buffer_snapshot);

        let snapshot = &mut self.snapshot;
        let mut inlay_edits = Vec::with_capacity(buffer_edits.len());
        let mut new_transforms = SumTree::default();
        let mut cursor = snapshot
            .transforms
            .cursor::<Dimensions<MultiBufferOffset, InlayOffset>>(());
        let mut buffer_edits_iter = buffer_edits.iter().peekable();

        while let Some(buffer_edit) = buffer_edits_iter.next() {
            new_transforms.append(cursor.slice(&buffer_edit.old.start, Bias::Left), ());
            if let Some(Transform::Isomorphic(transform)) = cursor.item()
                && cursor.end().0 == buffer_edit.old.start
            {
                push_isomorphic(&mut new_transforms, *transform);
                cursor.next();
            }

            let old_start = InlayOffset::new(
                cursor.start().1.get()
                    + buffer_edit
                        .old
                        .start
                        .get()
                        .saturating_sub(cursor.start().0.get()),
            );
            cursor.seek(&buffer_edit.old.end, Bias::Right);
            let old_end = InlayOffset::new(
                cursor.start().1.get()
                    + buffer_edit
                        .old
                        .end
                        .get()
                        .saturating_sub(cursor.start().0.get()),
            );

            let prefix_start = MultiBufferOffset::new(new_transforms.summary().input.len);
            let prefix_end = buffer_edit.new.start;
            push_isomorphic(
                &mut new_transforms,
                summary_for(&buffer_snapshot, prefix_start, prefix_end),
            );
            let new_start = InlayOffset::new(new_transforms.summary().output.len);

            let start_ix = self
                .inlays
                .partition_point(|inlay| inlay.position < buffer_edit.new.start);

            for inlay in &self.inlays[start_ix..] {
                let buffer_offset = inlay.position;
                if buffer_offset > buffer_edit.new.end {
                    break;
                }
                let prefix_start = MultiBufferOffset::new(new_transforms.summary().input.len);
                push_isomorphic(
                    &mut new_transforms,
                    summary_for(&buffer_snapshot, prefix_start, buffer_offset),
                );
                new_transforms.push(Transform::Inlay(inlay.clone()), ());
            }

            let transform_start = MultiBufferOffset::new(new_transforms.summary().input.len);
            push_isomorphic(
                &mut new_transforms,
                summary_for(&buffer_snapshot, transform_start, buffer_edit.new.end),
            );
            let new_end = InlayOffset::new(new_transforms.summary().output.len);
            inlay_edits.push(InlayEdit::new(old_start..old_end, new_start..new_end));

            if buffer_edits_iter
                .peek()
                .is_none_or(|edit| edit.old.start >= cursor.end().0)
            {
                let transform_start = MultiBufferOffset::new(new_transforms.summary().input.len);
                let transform_end = MultiBufferOffset::new(
                    buffer_edit.new.end.get()
                        + cursor
                            .end()
                            .0
                            .get()
                            .saturating_sub(buffer_edit.old.end.get()),
                );
                push_isomorphic(
                    &mut new_transforms,
                    summary_for(&buffer_snapshot, transform_start, transform_end),
                );
                cursor.next();
            }
        }

        new_transforms.append(cursor.suffix(), ());
        drop(cursor);
        if new_transforms.is_empty() {
            new_transforms.push(Transform::Isomorphic(Default::default()), ());
        }

        snapshot.transforms = new_transforms;
        snapshot.buffer = buffer_snapshot;
        snapshot.line_inlays = line_inlays_from_inlays(&snapshot.buffer, &self.inlays);
        // 注入配置版本只随 inlay 配置推进；文本编辑不改变它。
        if inlay_changed {
            snapshot.version += 1;
        }
        (snapshot.clone(), inlay_edits)
    }

    /// 当前 inlay 配置；供上层判断是否需要重新同步。
    pub(super) fn inlays(&self) -> &[Inlay] {
        &self.inlays
    }

    fn sort_inlays(&mut self, _buffer: &MultiBufferSnapshot) {
        self.inlays.sort_by_key(|inlay| inlay.position);
    }
}

fn summary_for(
    buffer: &MultiBufferSnapshot,
    start: MultiBufferOffset,
    end: MultiBufferOffset,
) -> MBTextSummary {
    if start == end {
        return MBTextSummary::default();
    }
    buffer
        .text_summary_for_range(MultiBufferRange::new(start, end).expect("投影摘要范围必须有序"))
        .unwrap_or_default()
}

fn text_summary_for(buffer: &MultiBufferSnapshot) -> MBTextSummary {
    summary_for(buffer, MultiBufferOffset::new(0), buffer.len_bytes())
}

fn push_isomorphic(transforms: &mut SumTree<Transform>, summary: MBTextSummary) {
    if summary.len == 0 {
        return;
    }
    transforms.push(Transform::Isomorphic(summary), ());
}

#[cfg(test)]
#[path = "test/inlay_map.rs"]
mod test;
