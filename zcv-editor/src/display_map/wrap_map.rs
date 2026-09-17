//! DisplayMap 的软换行（soft wrap）层。
//!
//! WrapMap 在 TabMap 之上，把超过指定像素宽度的逻辑行拆成多个显示行。
//! 换行点由文本系统对展开后的整行文本进行 shaping，再按词边界优先、长词硬断和首行缩进继承规则计算。
//! 续行的视觉缩进是一段"假空格"，作为显示文本的前缀参与布局、命中测试与坐标换算，因此渲染端无需为续行做任何特殊定位。
//!
//! 与 FoldMap 一样，WrapMap 用 `SumTree<Transform>` 维护"输入 tab 行 → 输出显示行"的拓扑：Isomorphic 段把连续不换行行合并，Wrap 段把单个宽行拆成 `wrap_points.len() + 1` 个显示行。
//! 折叠与换行是正交的两层变换：折叠先塌缩文本，换行再按像素宽度切分。

use std::ops::Range;
use std::sync::Arc;

use gpui::{Font, Pixels, TextRun, TextSystem, WindowTextSystem};
use sum_tree::{Bias, ContextLessSummary, Dimension, Dimensions, Item, SumTree};
use unicode_segmentation::UnicodeSegmentation;
use zcv_multi_buffer::MultiBufferSnapshot;
use zcv_text::{ByteOffset, CoordinateError, Line, LogicalColumn, Position, TextRange};

use super::chunk::{Chunk, ChunkBase, ChunkText, FoldChunks, HighlightStyles, InlayChunks};
use super::display_width::DisplayColumn;
use super::error::DisplayMapResult;
use super::fold_map::{
    FoldBias, FoldEdit, FoldRowSegment, FoldRowSegmentKind, LogicalPoint, LogicalProjection,
    LogicalRange, ProjectedLineIndex, ProjectedPoint, ProjectedRange, StreamProjectedKind,
};
use super::line_stream::StreamLineSource;
use super::tab_map::{TabSnapshot, advance_display_column, byte_for_display_column, line_content};
use super::{DisplayPoint, DisplayRow};

/// 换行点：行内容（已剥 `\r\n`）内的半开字节分界与下一续行的假空格数。
///
/// 显示行 i 的文本是 `content[prev_ix..ix]`，其中 `prev_ix` 是前一个换行点（首段为 0）；
/// 显示行 i + 1 以 `indent` 个假空格开头。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WrapPointInfo {
    byte_ix: usize,
    indent: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransformKind {
    Isomorphic,
    Wrap,
}

/// 输入 tab 行 → 输出显示行的变换。
///
/// - Isomorphic：n 个 tab 行 → n 个显示行（连续不换行行合并）；
/// - Wrap：1 个 tab 行 → `wrap_points.len() + 1` 个显示行。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Transform {
    kind: TransformKind,
    input_lines: usize,
    /// 换行点共享存储：克隆 Transform（增量重建时大量发生）只增加引用计数，不深拷贝换行点。
    wrap_points: Arc<[WrapPointInfo]>,
}

impl Transform {
    fn isomorphic(input_lines: usize) -> Self {
        Self {
            kind: TransformKind::Isomorphic,
            input_lines,
            wrap_points: Vec::new().into(),
        }
    }

    fn output_rows(&self) -> usize {
        match self.kind {
            TransformKind::Isomorphic => self.input_lines,
            TransformKind::Wrap => self.wrap_points.len() + 1,
        }
    }
}

impl Item for Transform {
    type Summary = TransformSummary;

    fn summary(&self, (): ()) -> Self::Summary {
        TransformSummary {
            input_lines: self.input_lines,
            output_rows: self.output_rows(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TransformSummary {
    input_lines: usize,
    output_rows: usize,
}

impl ContextLessSummary for TransformSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.input_lines += summary.input_lines;
        self.output_rows += summary.output_rows;
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct InputLines(usize);

impl<'a> Dimension<'a, TransformSummary> for InputLines {
    fn zero((): ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &'a TransformSummary, (): ()) {
        self.0 += summary.input_lines;
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct OutputRows(usize);

impl<'a> Dimension<'a, TransformSummary> for OutputRows {
    fn zero((): ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &'a TransformSummary, (): ()) {
        self.0 += summary.output_rows;
    }
}

type InputToOutput = Dimensions<InputLines, OutputRows>;
type OutputToInput = Dimensions<OutputRows, InputLines>;

/// 显示行对应的行片段信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WrapFragment {
    pub(super) tab_row: usize,
    pub(super) kind: WrapFragmentKind,
    /// 行内容（已剥 `\r\n`）内的半开字节区间。
    pub(super) byte_range: Range<usize>,
    /// 该显示行开头的假空格数（逻辑行首显示行为 0）。
    pub(super) indent: usize,
    /// 该显示行在所属逻辑行内的序号（0 = 逻辑行首显示行，gutter 行号在此）。
    pub(super) fragment_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WrapFragmentKind {
    /// 文本行（携带对应的 buffer 行来源）。
    Text(StreamLineSource),
}

/// Wrap 层输出的行元数据。
///
/// 文本本身由下游 `FoldSnapshot`/`TabSnapshot` 在消费 chunk 时按投影行读取；
/// 游标只传递坐标和来源，不在每一行物化 `Cow`/`Arc` 文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WrapRowKind {
    Text {
        source: StreamLineSource,
        projected_line: usize,
        byte_range: Range<usize>,
        global_byte_start: usize,
        fragment_index: usize,
        indent: usize,
    },
}

/// 按显示行顺序消费 Wrap 快照的游标。
///
/// 游标只在起点定位一次，之后通过 `next` 连续推进 transform；
/// 上层的 Block 游标跳过虚拟块时也只允许向前 seek，从而不会在每一行重新建立换行映射。
pub(super) struct WrapRows<'a> {
    snapshot: &'a WrapSnapshot,
    cursor: sum_tree::Cursor<'a, 'static, Transform, OutputToInput>,
    row: usize,
    end: usize,
}

impl<'a> WrapRows<'a> {
    pub(super) fn new(snapshot: &'a WrapSnapshot, start: usize, end: usize) -> Self {
        let end = end.min(snapshot.transforms.summary().output_rows);
        let mut cursor = snapshot.transforms.cursor::<OutputToInput>(());
        cursor.seek(&OutputRows(start), Bias::Right);
        Self {
            snapshot,
            cursor,
            row: start,
            end,
        }
    }

    pub(super) fn seek_forward(&mut self, row: usize) {
        debug_assert!(row >= self.row);
        if row > self.row {
            self.cursor.seek_forward(&OutputRows(row), Bias::Right);
            self.row = row;
        }
    }

    pub(super) fn next(&mut self) -> Option<WrapRowKind> {
        if self.row >= self.end {
            return None;
        }
        let Some(transform) = self.cursor.item() else {
            // 空投影可能保留逻辑行计数，但没有可消费的变换项。
            // 将游标标记为耗尽，避免初始化阶段把不存在的行当作有效显示片段。
            self.row = self.end;
            return None;
        };
        let transform_start = *self.cursor.start();
        let Some(fragment) = self
            .snapshot
            .fragment_in_transform(transform, transform_start, self.row)
            .ok()
        else {
            self.row += 1;
            return self.next();
        };
        let Some(row) = self.snapshot.row_kind(fragment).ok() else {
            self.row += 1;
            return self.next();
        };
        self.row += 1;
        if self.row >= self.cursor.end().0.0 {
            self.cursor.next();
        }
        Some(row)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WrapSnapshot {
    tab_snapshot: TabSnapshot,
    transforms: SumTree<Transform>,
    /// 是否处于软换行模式（false = 透传，显示行 == tab 行）。
    wrapped: bool,
    version: u64,
}

impl WrapSnapshot {
    pub(crate) fn tab_snapshot(&self) -> &TabSnapshot {
        &self.tab_snapshot
    }

    pub(super) fn buffer_snapshot(&self) -> &MultiBufferSnapshot {
        self.tab_snapshot.buffer_snapshot()
    }

    #[cfg(test)]
    pub(super) const fn version(&self) -> u64 {
        self.version
    }

    pub(super) fn line_count(&self) -> usize {
        self.transforms.summary().output_rows
    }

    /// 返回 Wrap 投影行对应的 Tab 投影行。
    ///
    /// 一个 Tab 投影行可能被软换行拆成多个 Wrap 行，因此调用方不能把 Wrap 行号直接当作 Tab 行号使用。
    pub(super) fn tab_row_for_wrap_row(&self, row: DisplayRow) -> DisplayMapResult<Line> {
        Ok(Line::new(self.display_row_to_fragment(row)?.tab_row))
    }

    pub(super) fn is_wrapped(&self) -> bool {
        self.wrapped
    }

    pub(super) fn offset_to_display_point(
        &self,
        offset: ByteOffset,
    ) -> DisplayMapResult<DisplayPoint> {
        let position = self
            .tab_snapshot
            .buffer_snapshot()
            .byte_to_position(offset)?;
        // fold 拓扑的输入坐标是流行号。
        let stream_line = self.tab_snapshot.stream().buffer_to_stream(position.line());
        self.logical_point_to_display_point(stream_line, position.column())
    }

    pub(super) fn display_point_to_offset(
        &self,
        point: DisplayPoint,
    ) -> DisplayMapResult<ByteOffset> {
        self.display_point_to_offset_with_bias(point, FoldBias::Left)
    }

    pub(super) fn display_point_to_offset_with_bias(
        &self,
        point: DisplayPoint,
        bias: FoldBias,
    ) -> DisplayMapResult<ByteOffset> {
        let fragment = self.display_row_to_fragment(point.row())?;
        match fragment.kind {
            WrapFragmentKind::Text(_source) => {
                let buffer = self.tab_snapshot.buffer_snapshot();
                let tab_row = Line::new(fragment.tab_row);
                let line_start = self
                    .tab_snapshot
                    .line_byte_range(tab_row)
                    .ok_or(CoordinateError::LineOutOfBounds(tab_row))?
                    .start
                    .get();
                let text = self
                    .tab_snapshot
                    .line_text(tab_row)
                    .ok_or(CoordinateError::LineOutOfBounds(tab_row))?;
                // 折叠合并行：按段映射显示列 → buffer 字节。
                let fold = self.tab_snapshot.fold_snapshot();
                if let Some(segments) =
                    fold.fold_row_segments(ProjectedLineIndex::new(fragment.tab_row))
                {
                    let content = line_content(text.as_ref());
                    let local = byte_for_display_column(
                        &content[fragment.byte_range.clone()],
                        fragment.indent,
                        point.column().get(),
                        buffer.config(),
                    );
                    return self.merged_byte_to_offset(
                        &segments,
                        fragment.byte_range.start + local,
                        bias,
                    );
                }
                let content = line_content(text.as_ref());
                let byte_range = fragment.byte_range;
                let local = byte_for_display_column(
                    &content[byte_range.clone()],
                    fragment.indent,
                    point.column().get(),
                    buffer.config(),
                );
                // 投影行内偏移逆投影回原始行内偏移（注入段内吸附到锚定后）。
                let stream_line = self
                    .tab_snapshot
                    .stream_line_for_projected(tab_row)
                    .ok_or(CoordinateError::LineOutOfBounds(tab_row))?;
                let inlay = fold.inlay_snapshot();
                let projected_byte = byte_range.start + local;
                let original_byte = inlay.to_original_offset(stream_line, projected_byte);
                Ok(ByteOffset::new(line_start + original_byte))
            }
        }
    }

    /// 折叠合并行内合并字节 → buffer 字节。
    ///
    /// anchor 段经行内逆投影；占位符吸附折叠起点（右箭头一步跨过折叠，左箭头可回 anchor 行尾）；
    /// 尾段映射到 close 行的真实字节。
    fn merged_byte_to_offset(
        &self,
        segments: &[FoldRowSegment],
        merged_byte: usize,
        bias: FoldBias,
    ) -> DisplayMapResult<ByteOffset> {
        let inlay = self.tab_snapshot.fold_snapshot().inlay_snapshot();
        let anchor = &segments[0];
        let placeholder = &segments[1];
        let tail = &segments[2];
        if merged_byte < anchor.merged_range.end {
            let FoldRowSegmentKind::Text { stream_line, .. } = &anchor.kind else {
                unreachable!("折叠合并行首段必须是 anchor 文本段");
            };
            let original = inlay.to_original_offset(*stream_line, merged_byte);
            return self.stream_offset(*stream_line, original);
        }
        if merged_byte < placeholder.merged_range.end {
            // 占位符列按选区方向吸附到折叠起点或终点；这样拖拽经过折叠时，隐藏内容会整体纳入选区。
            if bias == FoldBias::Right {
                let FoldRowSegmentKind::Text {
                    stream_line,
                    projected_range,
                } = &tail.kind
                else {
                    unreachable!("折叠合并行尾段必须是 close 文本段");
                };
                let original = inlay.to_original_offset(*stream_line, projected_range.start);
                return self.stream_offset(*stream_line, original);
            }
            let FoldRowSegmentKind::Text { stream_line, .. } = &anchor.kind else {
                unreachable!("折叠合并行首段必须是 anchor 文本段");
            };
            let anchor_end = inlay.to_original_offset(*stream_line, anchor.merged_range.end);
            return self.stream_offset(*stream_line, anchor_end);
        }
        let FoldRowSegmentKind::Text {
            stream_line,
            projected_range,
        } = &tail.kind
        else {
            unreachable!("折叠合并行尾段必须是 close 文本段");
        };
        let tail_projected = projected_range.start + (merged_byte - tail.merged_range.start);
        let original = inlay.to_original_offset(*stream_line, tail_projected);
        self.stream_offset(*stream_line, original)
    }

    fn stream_offset(&self, stream_line: Line, original: usize) -> DisplayMapResult<ByteOffset> {
        let inlay = self.tab_snapshot.fold_snapshot().inlay_snapshot();
        let range = inlay
            .line_byte_range(stream_line)
            .ok_or(CoordinateError::LineOutOfBounds(stream_line))?;
        Ok(ByteOffset::new(range.start.get() + original))
    }

    pub(super) fn rows(&self, start: usize, end: usize) -> WrapRows<'_> {
        WrapRows::new(self, start, end.min(self.line_count()))
    }

    /// Tab 投影行的内容长度，不读取或拼接投影整行。
    fn content_len_for_tab_row(&self, tab_row: usize) -> DisplayMapResult<usize> {
        let line = Line::new(tab_row);
        let fold = self.tab_snapshot.fold_snapshot();
        if let Some(segments) = fold.fold_row_segments(ProjectedLineIndex::new(tab_row)) {
            return Ok(segments
                .last()
                .expect("折叠合并行必须至少包含一个段")
                .merged_range()
                .end);
        }
        let stream_line = self
            .tab_snapshot
            .stream_line_for_projected(line)
            .ok_or(CoordinateError::LineOutOfBounds(line))?;
        fold.inlay_snapshot()
            .projected_line_content_metrics(stream_line)
            .map(|metrics| metrics.0)
            .ok_or_else(|| CoordinateError::LineOutOfBounds(line).into())
    }

    fn fragment_in_transform(
        &self,
        transform: &Transform,
        transform_start: OutputToInput,
        row: usize,
    ) -> DisplayMapResult<WrapFragment> {
        let output_start = transform_start.0.0;
        let input_start = transform_start.1.0;
        match transform.kind {
            TransformKind::Isomorphic => {
                let tab_row = input_start + row - output_start;
                let kind = self.projected_kind(tab_row)?;
                let content_len = self.content_len_for_tab_row(tab_row)?;
                Ok(WrapFragment {
                    tab_row,
                    kind,
                    byte_range: 0..content_len,
                    indent: 0,
                    fragment_index: 0,
                })
            }
            TransformKind::Wrap => {
                let kind = self.projected_kind(input_start)?;
                let content_len = self.content_len_for_tab_row(input_start)?;
                let fragment_index = row - output_start;
                Ok(WrapFragment {
                    tab_row: input_start,
                    kind,
                    byte_range: fragment_byte_range(
                        &transform.wrap_points,
                        fragment_index,
                        content_len,
                    ),
                    indent: fragment_index
                        .checked_sub(1)
                        .map_or(0, |index| transform.wrap_points[index].indent as usize),
                    fragment_index,
                })
            }
        }
    }

    fn row_kind(&self, fragment: WrapFragment) -> DisplayMapResult<WrapRowKind> {
        match fragment.kind {
            WrapFragmentKind::Text(source) => {
                let tab_row = Line::new(fragment.tab_row);
                let line_range = self
                    .tab_snapshot
                    .line_byte_range(tab_row)
                    .ok_or(CoordinateError::LineOutOfBounds(tab_row))?;
                Ok(WrapRowKind::Text {
                    source,
                    projected_line: fragment.tab_row,
                    byte_range: fragment.byte_range,
                    global_byte_start: line_range.start.get(),
                    fragment_index: fragment.fragment_index,
                    indent: fragment.indent,
                })
            }
        }
    }

    pub(super) fn project_text_range(
        &self,
        range: TextRange,
    ) -> DisplayMapResult<Vec<ProjectedRange>> {
        let buffer = self.tab_snapshot.buffer_snapshot();
        let logical = LogicalRange::new(
            LogicalPoint::from(buffer.byte_to_position(range.start())?),
            LogicalPoint::from(buffer.byte_to_position(range.end())?),
        )?;
        if logical.is_empty() {
            return Ok(Vec::new());
        }
        let fold = self.tab_snapshot.fold_snapshot();
        // 折叠内端点按 bias 投影：起点吸附折叠起点列，终点吸附折叠终点列。
        let start = self.projected_point_to_range_point(
            fold.logical_to_projected_point(logical.start(), FoldBias::Left)?,
        )?;
        let end = self.projected_point_to_range_point(
            fold.logical_to_projected_point(logical.end(), FoldBias::Right)?,
        )?;
        if start.0 > end.0 || (start.0 == end.0 && start.1 > end.1) {
            return Ok(Vec::new());
        }

        let breakpoints = [start, end];

        breakpoints
            .windows(2)
            .filter(|window| window[0] != window[1])
            .map(|window| {
                ProjectedRange::new(
                    ProjectedPoint::new(
                        ProjectedLineIndex::new(window[0].0.get()),
                        LogicalColumn::new(window[0].1),
                    ),
                    ProjectedPoint::new(
                        ProjectedLineIndex::new(window[1].0.get()),
                        LogicalColumn::new(window[1].1),
                    ),
                )
                .map_err(Into::into)
            })
            .collect()
    }

    /// 光标所在的显示行行首（列 0）对应的字节偏移。
    pub(super) fn beginning_of_row(&self, offset: ByteOffset) -> DisplayMapResult<ByteOffset> {
        let point = self.offset_to_display_point(offset)?;
        self.display_point_to_offset(DisplayPoint::new(point.row(), DisplayColumn::ZERO))
    }

    /// 光标所在的显示行行尾（本段末尾，不含换行符）对应的字节偏移。
    pub(super) fn end_of_row(&self, offset: ByteOffset) -> DisplayMapResult<ByteOffset> {
        let point = self.offset_to_display_point(offset)?;
        let fragment = self.display_row_to_fragment(point.row())?;
        match fragment.kind {
            WrapFragmentKind::Text(_source) => {
                let tab_row = Line::new(fragment.tab_row);
                let line_start = self
                    .tab_snapshot
                    .line_byte_range(tab_row)
                    .ok_or(CoordinateError::LineOutOfBounds(tab_row))?
                    .start
                    .get();
                // 折叠合并行：行尾 = 合并文本末尾（close 行内容末尾）。
                let fold = self.tab_snapshot.fold_snapshot();
                if let Some(segments) =
                    fold.fold_row_segments(ProjectedLineIndex::new(fragment.tab_row))
                {
                    return self.merged_byte_to_offset(
                        &segments,
                        fragment.byte_range.end,
                        FoldBias::Left,
                    );
                }
                // 片段终点（投影偏移）逆投影回原始行内偏移。
                let stream_line = self
                    .tab_snapshot
                    .stream_line_for_projected(tab_row)
                    .ok_or(CoordinateError::LineOutOfBounds(tab_row))?;
                let inlay = fold.inlay_snapshot();
                let original_end = inlay.to_original_offset(stream_line, fragment.byte_range.end);
                Ok(ByteOffset::new(line_start + original_end))
            }
        }
    }

    /// 显示行 → (tab 行, 片段信息)。
    fn display_row_to_fragment(&self, row: DisplayRow) -> DisplayMapResult<WrapFragment> {
        let (start, _, transform) =
            self.transforms
                .find::<OutputToInput, _>((), &OutputRows(row.get()), Bias::Right);
        let transform = transform.ok_or(CoordinateError::LineOutOfBounds(Line::new(row.get())))?;
        let input_start = start.1.0;
        let output_start = start.0.0;
        match transform.kind {
            TransformKind::Isomorphic => {
                let tab_row = input_start + (row.get() - output_start);
                let kind = self.projected_kind(tab_row)?;
                let content_len = self.content_len_for_tab_row(tab_row)?;
                Ok(WrapFragment {
                    tab_row,
                    kind,
                    byte_range: 0..content_len,
                    indent: 0,
                    fragment_index: 0,
                })
            }
            TransformKind::Wrap => {
                let kind = self.projected_kind(input_start)?;
                let content_len = self.content_len_for_tab_row(input_start)?;
                let fragment_index = row.get() - output_start;
                Ok(WrapFragment {
                    tab_row: input_start,
                    kind,
                    byte_range: fragment_byte_range(
                        &transform.wrap_points,
                        fragment_index,
                        content_len,
                    ),
                    indent: fragment_index
                        .checked_sub(1)
                        .map_or(0, |i| transform.wrap_points[i].indent as usize),
                    fragment_index,
                })
            }
        }
    }

    fn projected_kind(&self, tab_row: usize) -> DisplayMapResult<WrapFragmentKind> {
        match self.tab_snapshot.projected_kind(Line::new(tab_row)) {
            Some(StreamProjectedKind::Text(source)) => Ok(WrapFragmentKind::Text(source)),
            None => Err(CoordinateError::LineOutOfBounds(Line::new(tab_row)).into()),
        }
    }

    /// 逻辑行内的点 → 显示点；列 = 显示行内 display column（含假空格缩进）。
    fn logical_point_to_display_point(
        &self,
        line: Line,
        column: LogicalColumn,
    ) -> DisplayMapResult<DisplayPoint> {
        let fold = self.tab_snapshot.fold_snapshot();
        // 隐藏点吸附折叠起点列（光标在折叠内的默认落点）。
        let point =
            fold.logical_to_projected_point(LogicalPoint::new(line, column), FoldBias::Left)?;
        self.projected_point_to_display_point(point)
    }

    /// 投影点（tab 行 + 逻辑列）→ 显示点。
    /// 投影点列 → 行内投影字节。
    ///
    /// 折叠合并行按合并文本字符列换算（anchor/占位符/尾段都在行文本内）；
    /// 普通行经原始字节逆投影（含行内提示注入前缀）。
    fn projected_column_to_byte(
        &self,
        line: Line,
        column: LogicalColumn,
    ) -> DisplayMapResult<usize> {
        let fold = self.tab_snapshot.fold_snapshot();
        let text = self
            .tab_snapshot
            .line_text(line)
            .ok_or(CoordinateError::LineOutOfBounds(line))?;
        let content = line_content(text.as_ref());
        if fold.is_fold_row(ProjectedLineIndex::new(line.get())) {
            return Ok(byte_after_chars(content, column.get()));
        }
        let buffer = self.tab_snapshot.buffer_snapshot();
        let line_start = self
            .tab_snapshot
            .line_byte_range(line)
            .ok_or(CoordinateError::LineOutOfBounds(line))?
            .start
            .get();
        let stream_line = self
            .tab_snapshot
            .stream_line_for_projected(line)
            .ok_or(CoordinateError::LineOutOfBounds(line))?;
        let inlay = fold.inlay_snapshot();
        let buffer_line = match inlay.source(stream_line) {
            Some(source) => Line::new(source.line()),
            _ => return Err(CoordinateError::LineOutOfBounds(line).into()),
        };
        let target_byte = buffer
            .position_to_byte(Position::new(buffer_line, column))?
            .get()
            - line_start;
        Ok(inlay.to_projected_offset(stream_line, target_byte))
    }

    fn projected_point_to_display_point(
        &self,
        point: ProjectedPoint,
    ) -> DisplayMapResult<DisplayPoint> {
        let tab_row = point.line().get();
        let line = Line::new(tab_row);
        let buffer = self.tab_snapshot.buffer_snapshot();
        // 投影文本（含行内提示注入）；目标列 → 行内投影字节。
        let text = self
            .tab_snapshot
            .line_text(line)
            .ok_or(CoordinateError::LineOutOfBounds(line))?;
        let content = line_content(text.as_ref());
        let target_projected = self.projected_column_to_byte(line, point.column())?;
        let (input_start, output_start, transform) = self.transform_for_tab_row(tab_row)?;
        let (fragment_index, fragment_start, indent) = match transform.kind {
            TransformKind::Isomorphic => (tab_row - input_start, 0, 0),
            TransformKind::Wrap => {
                let fragment_index =
                    fragment_index_for_byte(&transform.wrap_points, target_projected);
                (
                    fragment_index,
                    fragment_index
                        .checked_sub(1)
                        .map_or(0, |i| transform.wrap_points[i].byte_ix),
                    fragment_index
                        .checked_sub(1)
                        .map_or(0, |i| transform.wrap_points[i].indent as usize),
                )
            }
        };
        // 片段内的显示列从缩进后的列开始累加，tab 对齐基于显示行内列。
        let column = content[fragment_start..target_projected]
            .graphemes(true)
            .fold(indent, |column, grapheme| {
                advance_display_column(column, grapheme, buffer.config())
            });
        Ok(DisplayPoint::new(
            DisplayRow::new(output_start + fragment_index),
            DisplayColumn::new(column),
        ))
    }

    fn transform_for_tab_row(
        &self,
        tab_row: usize,
    ) -> DisplayMapResult<(usize, usize, &Transform)> {
        let (start, _, transform) =
            self.transforms
                .find::<InputToOutput, _>((), &InputLines(tab_row), Bias::Right);
        let transform = transform.ok_or(CoordinateError::LineOutOfBounds(Line::new(tab_row)))?;
        Ok((start.0.0, start.1.0, transform))
    }

    /// 选区起终点（投影点）→ (显示行, 显示行内字符列)；列按字符计数（含假空格），
    /// 与渲染端 `column_to_byte` 的语义一致。
    fn projected_point_to_range_point(
        &self,
        point: ProjectedPoint,
    ) -> DisplayMapResult<(DisplayRow, usize)> {
        let tab_row = point.line().get();
        let line = Line::new(tab_row);
        let buffer = self.tab_snapshot.buffer_snapshot();
        let fold = self.tab_snapshot.fold_snapshot();
        let merged = fold.is_fold_row(ProjectedLineIndex::new(tab_row));
        let target_projected = self.projected_column_to_byte(line, point.column())?;
        let (input_start, output_start, transform) = self.transform_for_tab_row(tab_row)?;
        let (fragment_index, fragment_start, indent) = match transform.kind {
            TransformKind::Isomorphic => (tab_row - input_start, 0, 0),
            TransformKind::Wrap => {
                let fragment_index =
                    fragment_index_for_byte(&transform.wrap_points, target_projected);
                (
                    fragment_index,
                    fragment_index
                        .checked_sub(1)
                        .map_or(0, |i| transform.wrap_points[i].byte_ix),
                    fragment_index
                        .checked_sub(1)
                        .map_or(0, |i| transform.wrap_points[i].indent as usize),
                )
            }
        };
        // 片段起点列：合并行按合并文本字符数；普通行逆投影回原始字节 → 起始逻辑列。
        let column_base = if merged {
            let text = self
                .tab_snapshot
                .line_text(line)
                .ok_or(CoordinateError::LineOutOfBounds(line))?;
            line_content(text.as_ref())[..fragment_start]
                .chars()
                .count()
        } else {
            let line_start = self
                .tab_snapshot
                .line_byte_range(line)
                .ok_or(CoordinateError::LineOutOfBounds(line))?
                .start
                .get();
            let stream_line = self
                .tab_snapshot
                .stream_line_for_projected(line)
                .ok_or(CoordinateError::LineOutOfBounds(line))?;
            let inlay = fold.inlay_snapshot();
            let original_start = inlay.to_original_offset(stream_line, fragment_start);
            buffer
                .byte_to_position(ByteOffset::new(line_start + original_start))
                .map_or(0, |position| position.column().get())
        };
        Ok((
            DisplayRow::new(output_start + fragment_index),
            indent + (point.column().get() - column_base),
        ))
    }
}

/// 由「换行输入行区间 + 重测后的输出行数」派生显示行编辑。
///
/// 多个区间按输入行升序处理，前一次重排的输出行差会平移后续区间的显示坐标。
/// 只有输出行数量真正变化的区间才记录；空列表精确表示显示行布局未变。
fn wrap_edits(
    measure: &SumTree<Transform>,
    old_ranges: &[Range<usize>],
    new_lengths: &[usize],
) -> Vec<WrapEdit> {
    let mut result = Vec::new();
    let mut delta = 0isize;
    for (old_rows, new_len) in old_ranges.iter().zip(new_lengths) {
        let old_before = output_rows_before(measure, old_rows.start);
        let old_after = output_rows_before(measure, old_rows.end);
        let start = (old_before as isize + delta) as usize;
        let old_range = start..start + (old_after - old_before);
        let new_range = start..start + new_len;
        if old_range != new_range {
            result.push(WrapEdit {
                old: old_range,
                new: new_range,
            });
        }
        delta += *new_len as isize - (old_after - old_before) as isize;
    }
    result
}

/// 文本中第 `chars` 个字符的字节偏移（超出末尾返回文本长度）。
fn byte_after_chars(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(byte, _)| byte)
}
/// 一次换行重排影响的显示行区间（换行输出行空间）。
///
/// 无重排时列表为空，因此「空」精确表示显示行布局未变；有重排时才记录旧/新区间，
/// 供 BlockMap 判断块位置是否需要重建。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WrapEdit {
    pub(super) old: Range<usize>,
    pub(super) new: Range<usize>,
}

/// 给定输入行之前累计的输出行数（换行树的 InputToOutput 维度）。
///
/// 非零输出行的 item 只有 Wrap（input_lines 恒为 1），因此落在 item 内部的行只可能
/// 属于 Isomorphic 段，其增量与输入行偏移一致；用 `Bias::Right` 让区间终点的边界行落到后继 item。
fn output_rows_before(tree: &SumTree<Transform>, input_row: usize) -> usize {
    let mut cursor = tree.cursor::<InputToOutput>(());
    cursor.seek(&InputLines(input_row), Bias::Right);
    let start = cursor.start();
    start.1.0 + (input_row - start.0.0)
}

#[derive(Clone)]
pub(super) struct WrapMap {
    snapshot: WrapSnapshot,
    wrap_width: Option<Pixels>,
    font_with_size: Option<(Font, Pixels)>,
    /// 由 `set_wrap_width` 缓存；重排时使用同一个 text system 做整行 shaping。
    text_system: Option<Arc<TextSystem>>,
    /// 换行阶段的整行 shaping 缓存；与文本系统共享字体资源，但独立于窗口布局生命周期。
    window_text_system: Option<Arc<WindowTextSystem>>,
}

impl std::fmt::Debug for WrapMap {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WrapMap")
            .field("snapshot", &self.snapshot)
            .field("wrap_width", &self.wrap_width)
            .field("font_with_size", &self.font_with_size)
            .finish_non_exhaustive()
    }
}

impl WrapMap {
    pub(super) fn new(tab_snapshot: TabSnapshot) -> (Self, WrapSnapshot) {
        let transforms = isomorphic_tree(tab_snapshot.line_count());
        let snapshot = WrapSnapshot {
            tab_snapshot,
            transforms,
            wrapped: false,
            version: 0,
        };
        (
            Self {
                snapshot: snapshot.clone(),
                wrap_width: None,
                font_with_size: None,
                text_system: None,
                window_text_system: None,
            },
            snapshot,
        )
    }

    pub(super) fn snapshot(&self) -> &WrapSnapshot {
        &self.snapshot
    }

    /// 同步 tab 层变化。tab 版本未变时不做任何事；换行开启时按 fold edit 的
    /// 结构性变化全量重排或按 changed_lines 增量重排，关闭时重建单段透传。
    pub(super) fn sync(
        &mut self,
        tab_snapshot: TabSnapshot,
        fold_edits: &[FoldEdit],
    ) -> Vec<WrapEdit> {
        if tab_snapshot.version() == self.snapshot.tab_snapshot.version() {
            return Vec::new();
        }
        self.snapshot.tab_snapshot = tab_snapshot;
        let edits = if let Some(wrap_width) = self.wrap_width {
            // 结构编辑（折叠/展开、行内提示变化）按 FoldEdit 的旧/新输入行区间局部重排；
            // 覆盖全量的结构编辑自然退化为整段重建，不需要单独的“全量”分支。
            if fold_edits.iter().any(FoldEdit::is_structural) {
                self.update_structural(fold_edits, wrap_width)
            } else {
                let changed_lines: Vec<Line> = fold_edits
                    .iter()
                    .flat_map(|edit| edit.changed_lines().iter().copied())
                    .collect();
                self.update_inline(&changed_lines, wrap_width)
            }
        } else {
            self.set_isomorphic_all()
        };
        self.check_invariants();
        self.snapshot.version += 1;
        edits
    }

    /// 设置换行宽度与字体。只有 (宽度, 字体, 字号) 任一变化时才重建；
    /// 返回是否发生了变化。
    pub(super) fn set_wrap_width(
        &mut self,
        wrap_width: Option<Pixels>,
        font: Font,
        font_size: Pixels,
        text_system: Arc<TextSystem>,
    ) -> (bool, Vec<WrapEdit>) {
        let width_changed = wrap_width != self.wrap_width;
        let font_changed =
            self.font_with_size
                .as_ref()
                .is_some_and(|(cached_font, cached_size)| {
                    *cached_font != font || *cached_size != font_size
                });
        let text_system_changed = self
            .text_system
            .as_ref()
            .is_none_or(|cached| !Arc::ptr_eq(cached, &text_system));
        let needs_rewrap = width_changed
            || (font_changed && wrap_width.is_some())
            || (text_system_changed && wrap_width.is_some());
        if text_system_changed {
            self.window_text_system = Some(Arc::new(WindowTextSystem::new(text_system.clone())));
        }
        self.text_system = Some(text_system);
        if !needs_rewrap {
            return (false, Vec::new());
        }
        self.wrap_width = wrap_width;
        self.font_with_size = Some((font, font_size));
        let edits = match wrap_width {
            None => self.set_isomorphic_all(),
            Some(width) => self.rewrap_all(width),
        };
        self.snapshot.version += 1;
        (true, edits)
    }

    fn set_isomorphic_all(&mut self) -> Vec<WrapEdit> {
        let old_rows = self.snapshot.transforms.summary().output_rows;
        self.snapshot.transforms = isomorphic_tree(self.snapshot.tab_snapshot.line_count());
        self.snapshot.wrapped = false;
        self.check_invariants();
        vec![WrapEdit {
            old: 0..old_rows,
            new: 0..self.snapshot.transforms.summary().output_rows,
        }]
    }

    /// 结构编辑的局部重排：按 FoldEdit 的旧/新输入行区间替换换行变换。
    ///
    /// 未命中的前缀/后缀子树直接复用（Arc 共享）；被替换区间内的行重新测量换行。
    /// 覆盖全量的结构编辑会退化为整段重建，与 [`Self::rewrap_all`] 等价。
    fn update_structural(&mut self, fold_edits: &[FoldEdit], wrap_width: Pixels) -> Vec<WrapEdit> {
        let mut edits: Vec<(Range<usize>, Range<usize>)> = fold_edits
            .iter()
            .map(|edit| (edit.old_rows(), edit.new_rows()))
            .collect();
        edits.sort_by_key(|(old_rows, _)| old_rows.start);

        // 以输入行为维度的游标 splice：未命中的前缀/后缀子树直接复用（Arc 共享），
        // 只有与被替换行相交的边界 item 需要拆分，受影响行重新测量换行。
        let measure = self.snapshot.transforms.clone();
        let old_transforms = std::mem::replace(&mut self.snapshot.transforms, SumTree::new(()));
        let mut cursor = old_transforms.cursor::<InputLines>(());
        let mut new_tree = SumTree::new(());
        let mut buffered = Vec::new();
        let mut measured = Vec::with_capacity(edits.len());
        for (old_rows, new_rows) in &edits {
            new_tree.append(cursor.slice(&InputLines(old_rows.start), Bias::Left), ());

            // 起始边界：item 若从 old_rows.start 之前开始，保留 [item_start, old_rows.start) 部分。
            if let Some(transform) = cursor.item() {
                let transform_start = cursor.start().0;
                if transform_start < old_rows.start {
                    push_transform_slice(
                        &mut buffered,
                        transform,
                        old_rows.start - transform_start,
                    );
                }
            }

            // 丢弃 [old_rows.start, old_rows.end) 内的 item；跨过 end 的 item 保留尾部。
            let mut tail: Option<Transform> = None;
            while let Some(transform) = cursor.item() {
                let transform_start = cursor.start().0;
                if transform_start >= old_rows.end {
                    break;
                }
                let transform_end = transform_start + transform.input_lines;
                if transform_end > old_rows.end {
                    tail = Some(if transform.kind == TransformKind::Isomorphic {
                        Transform::isomorphic(transform_end - old_rows.end)
                    } else {
                        transform.clone()
                    });
                    cursor.next();
                    break;
                }
                cursor.next();
            }

            let measured_start = buffered.len();
            for tab_row in new_rows.clone() {
                self.push_wrap_transform(&mut buffered, tab_row, wrap_width);
            }
            measured.push(
                buffered[measured_start..]
                    .iter()
                    .map(Transform::output_rows)
                    .sum::<usize>(),
            );
            if let Some(tail) = tail {
                buffered.push(tail);
            }
            new_tree.extend(buffered.drain(..), ());
        }
        new_tree.append(cursor.suffix(), ());
        self.snapshot.transforms = new_tree;
        self.snapshot.wrapped = true;
        self.check_invariants();
        wrap_edits(
            &measure,
            &edits
                .iter()
                .map(|(old_rows, _)| old_rows.clone())
                .collect::<Vec<_>>(),
            &measured,
        )
    }

    /// 全量重建：对每个 tab 行重新计算换行点。
    fn rewrap_all(&mut self, wrap_width: Pixels) -> Vec<WrapEdit> {
        let old_rows = self.snapshot.transforms.summary().output_rows;
        let mut transforms = Vec::new();
        for tab_row in 0..self.snapshot.tab_snapshot.line_count() {
            self.push_wrap_transform(&mut transforms, tab_row, wrap_width);
        }
        self.snapshot.transforms = SumTree::from_iter(transforms, ());
        self.snapshot.wrapped = true;
        self.check_invariants();
        vec![WrapEdit {
            old: 0..old_rows,
            new: 0..self.snapshot.transforms.summary().output_rows,
        }]
    }

    /// 行级增量：只重排 changed_lines 对应的 tab 行，其余段落原样保留。
    fn update_inline(&mut self, changed_lines: &[Line], wrap_width: Pixels) -> Vec<WrapEdit> {
        let fold = self.snapshot.tab_snapshot.fold_snapshot();
        let mut rows: Vec<usize> = changed_lines
            .iter()
            .filter_map(|line| match fold.logical_to_projected(*line).ok()? {
                LogicalProjection::Visible(row) => Some(row.get()),
                LogicalProjection::Hidden => None,
            })
            .collect();
        rows.sort_unstable();
        rows.dedup();
        if rows.is_empty() {
            return Vec::new();
        }
        let row_edits = merge_ranges(&rows);

        // 以输入行为维度的游标 splice：未命中的前缀/后缀子树直接复用（Arc 共享），
        // 只有与被编辑行相交的边界 item 需要拆分，受影响行重新测量换行。
        let measure = self.snapshot.transforms.clone();
        let old_transforms = std::mem::replace(&mut self.snapshot.transforms, SumTree::new(()));
        let mut cursor = old_transforms.cursor::<InputLines>(());
        let mut new_tree = SumTree::new(());
        let mut buffered = Vec::new();
        let mut measured = Vec::with_capacity(row_edits.len());
        for edit in &row_edits {
            new_tree.append(cursor.slice(&InputLines(edit.start), Bias::Left), ());

            // 起始边界：item 若从 edit.start 之前开始，保留 [item_start, edit.start) 部分。
            if let Some(transform) = cursor.item() {
                let transform_start = cursor.start().0;
                if transform_start < edit.start {
                    push_transform_slice(&mut buffered, transform, edit.start - transform_start);
                }
            }

            // 丢弃 [edit.start, edit.end) 内的 item；跨过 edit.end 的 item 保留尾部。
            let mut tail: Option<Transform> = None;
            while let Some(transform) = cursor.item() {
                let transform_start = cursor.start().0;
                if transform_start >= edit.end {
                    break;
                }
                let transform_end = transform_start + transform.input_lines;
                if transform_end > edit.end {
                    tail = Some(if transform.kind == TransformKind::Isomorphic {
                        Transform::isomorphic(transform_end - edit.end)
                    } else {
                        transform.clone()
                    });
                    cursor.next();
                    break;
                }
                cursor.next();
            }

            let measured_start = buffered.len();
            for tab_row in edit.start..edit.end {
                self.push_wrap_transform(&mut buffered, tab_row, wrap_width);
            }
            measured.push(
                buffered[measured_start..]
                    .iter()
                    .map(Transform::output_rows)
                    .sum::<usize>(),
            );
            if let Some(tail) = tail {
                buffered.push(tail);
            }
            new_tree.extend(buffered.drain(..), ());
        }
        new_tree.append(cursor.suffix(), ());
        self.snapshot.transforms = new_tree;
        self.snapshot.wrapped = true;
        self.check_invariants();
        wrap_edits(&measure, &row_edits, &measured)
    }

    /// 计算单个 tab 行的换行变换并压入（相邻 Isomorphic 自动合并）。
    fn push_wrap_transform(
        &self,
        transforms: &mut Vec<Transform>,
        tab_row: usize,
        wrap_width: Pixels,
    ) {
        // 调用点保证 tab_row 落在当前 tab 行数内；越界说明换行层与 fold/tab 快照不一致，应直接暴露。
        let WrapFragmentKind::Text(_) = self
            .snapshot
            .projected_kind(tab_row)
            .expect("换行变换只能作用于已投影的文本行");
        let prepared = self
            .prepared_wrap_text(tab_row)
            .expect("已投影的文本行必须能建立塑形输入");
        let boundaries = self.wrap_points(prepared, wrap_width);
        if boundaries.is_empty() {
            // 无需软换行：一个输入行对应一个输出行。
            push_isomorphic(transforms, 1);
        } else {
            transforms.push(Transform {
                kind: TransformKind::Wrap,
                input_lines: 1,
                wrap_points: boundaries.into(),
            });
        }
    }

    /// 为单个投影行建立文字塑形输入。
    ///
    /// 软换行必须把当前行交给文字系统塑形；
    /// 这里直接消费 Fold/Inlay 连续 chunk，只保留塑形所需的一份临时文本，不先生成另一份投影整行。
    fn prepared_wrap_text(&self, tab_row: usize) -> DisplayMapResult<PreparedWrapText> {
        let tab = &self.snapshot.tab_snapshot;
        let fold = tab.fold_snapshot();
        let tab_width = tab.buffer_snapshot().config().tab.tab_width();
        if let Some(segments) = fold.fold_row_segments(ProjectedLineIndex::new(tab_row)) {
            let content_len = segments
                .last()
                .expect("折叠合并行必须至少包含一个段")
                .merged_range()
                .end;
            return Ok(PreparedWrapText::from_chunks(
                FoldChunks::new(
                    &segments,
                    fold.inlay_snapshot(),
                    HighlightStyles::default(),
                    0..content_len,
                ),
                tab_width,
            ));
        }
        let line = Line::new(tab_row);
        let stream_line = tab
            .stream_line_for_projected(line)
            .ok_or(CoordinateError::LineOutOfBounds(line))?;
        let inlay = fold.inlay_snapshot();
        let range = inlay
            .line_content_byte_range(stream_line)
            .ok_or(CoordinateError::LineOutOfBounds(line))?;
        let content_len = inlay
            .projected_line_content_metrics(stream_line)
            .ok_or(CoordinateError::LineOutOfBounds(line))?
            .0;
        Ok(PreparedWrapText::from_chunks(
            InlayChunks::new(
                ChunkText::Virtual {
                    snapshot: inlay.buffer_snapshot(),
                    range: range.clone(),
                },
                range.start.get(),
                inlay.line_inlays(stream_line),
                ChunkBase::ZERO,
                HighlightStyles::default(),
                0..content_len,
                true,
            ),
            tab_width,
        ))
    }

    /// 使用最终字形位置计算换行点，避免字符宽度估算与渲染 shaping 使用两套标准。
    fn wrap_points(&self, prepared: PreparedWrapText, wrap_width: Pixels) -> Vec<WrapPointInfo> {
        let window_text_system = self
            .window_text_system
            .as_ref()
            .expect("换行开启时必须先通过 set_wrap_width 缓存 shaping 系统");
        let (font, font_size) = self
            .font_with_size
            .as_ref()
            .expect("换行开启时必须先通过 set_wrap_width 缓存字体");
        if prepared.chars.is_empty() {
            return Vec::new();
        }
        let run = TextRun {
            len: prepared.text.len(),
            font: font.clone(),
            ..Default::default()
        };
        let shaped =
            window_text_system.shape_line(prepared.text.clone().into(), *font_size, &[run], None);

        let mut points = Vec::new();
        let mut first_non_whitespace = None;
        let mut indent = None;
        let mut indent_width = Pixels::ZERO;
        let mut last_candidate = None;
        let mut last_wrap = 0usize;
        let mut line_start = 0usize;
        let mut previous = '\0';

        for character in &prepared.chars {
            if is_word_char(character.ch) {
                if previous == ' ' && character.ch != ' ' && first_non_whitespace.is_some() {
                    last_candidate = Some(character.raw_start);
                }
            } else if character.ch != ' ' && first_non_whitespace.is_some() {
                last_candidate = Some(character.raw_start);
            }

            if character.ch != ' ' && first_non_whitespace.is_none() {
                first_non_whitespace = Some(character.raw_start);
            }

            let line_width = shaped.x_for_index(character.expanded_end)
                - shaped.x_for_index(line_start)
                + if last_wrap > 0 {
                    indent_width
                } else {
                    Pixels::ZERO
                };
            if line_width > wrap_width && character.raw_start > last_wrap {
                if indent.is_none()
                    && let Some(first_non_whitespace) = first_non_whitespace
                {
                    let indent_columns = prepared
                        .chars
                        .iter()
                        .take_while(|character| character.raw_start < first_non_whitespace)
                        .count()
                        .min(gpui::LineWrapper::MAX_INDENT as usize);
                    indent = Some(indent_columns);
                    indent_width =
                        shaped_space_width(window_text_system, font, *font_size, indent_columns);
                }

                let boundary = last_candidate
                    .filter(|candidate| *candidate > last_wrap)
                    .unwrap_or(character.raw_start);
                points.push(WrapPointInfo {
                    byte_ix: boundary,
                    indent: indent.unwrap_or(0) as u32,
                });
                last_wrap = boundary;
                line_start = prepared.expanded_start(boundary);
                last_candidate = None;
            }
            previous = character.ch;
        }

        points
    }

    fn check_invariants(&self) {
        #[cfg(debug_assertions)]
        {
            let tab_rows = self.snapshot.tab_snapshot.line_count();
            assert_eq!(self.snapshot.transforms.summary().input_lines, tab_rows);
            for transform in self.snapshot.transforms.iter() {
                match transform.kind {
                    TransformKind::Isomorphic => assert!(transform.input_lines > 0),
                    TransformKind::Wrap => {
                        assert_eq!(transform.input_lines, 1);
                        assert!(!transform.wrap_points.is_empty());
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
struct PreparedWrapChar {
    ch: char,
    raw_start: usize,
    expanded_start: usize,
    expanded_end: usize,
}

/// 把 tab 按渲染端的列规则展开，并保留投影文本到塑形文本的边界映射。
#[derive(Debug)]
struct PreparedWrapText {
    text: String,
    chars: Vec<PreparedWrapChar>,
}

impl PreparedWrapText {
    fn from_chunks<'a>(chunks: impl IntoIterator<Item = Chunk<'a>>, tab_width: usize) -> Self {
        let mut text = String::new();
        let mut chars = Vec::new();
        let mut column = 0usize;
        let mut raw_start = 0usize;
        for chunk in chunks {
            for ch in chunk.text.chars() {
                if ch == '\n' || ch == '\r' {
                    raw_start += ch.len_utf8();
                    continue;
                }
                let expanded_start = text.len();
                if ch == '\t' {
                    let width = tab_width - column % tab_width;
                    text.extend(std::iter::repeat_n(' ', width));
                    column += width;
                } else {
                    text.push(ch);
                    column += 1;
                }
                chars.push(PreparedWrapChar {
                    ch,
                    raw_start,
                    expanded_start,
                    expanded_end: text.len(),
                });
                raw_start += ch.len_utf8();
            }
        }
        Self { text, chars }
    }

    fn expanded_start(&self, raw_start: usize) -> usize {
        self.chars
            .iter()
            .find(|character| character.raw_start == raw_start)
            .map_or(self.text.len(), |character| character.expanded_start)
    }
}

fn shaped_space_width(
    text_system: &WindowTextSystem,
    font: &Font,
    font_size: Pixels,
    count: usize,
) -> Pixels {
    if count == 0 {
        return Pixels::ZERO;
    }
    let text = " ".repeat(count);
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        ..Default::default()
    };
    text_system
        .shape_line(text.into(), font_size, &[run], None)
        .width()
}

/// 与 GPUI LineWrapper 保持一致的断词分类；宽度判断由本模块的整行 shaping 提供。
fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(ch, '\u{00C0}'..='\u{00FF}')
        || matches!(ch, '\u{0100}'..='\u{017F}')
        || matches!(ch, '\u{0180}'..='\u{024F}')
        || matches!(ch, '\u{0400}'..='\u{04FF}')
        || matches!(ch, '\u{1E00}'..='\u{1EFF}')
        || matches!(ch, '\u{0300}'..='\u{036F}')
        || matches!(ch, '\u{0980}'..='\u{09FF}')
        || matches!(
            ch,
            '-' | '_'
                | '.'
                | '\''
                | '’'
                | '‘'
                | '$'
                | '%'
                | '@'
                | '#'
                | '^'
                | '~'
                | ','
                | '='
                | ':'
                | ';'
        )
        || matches!(ch, '!' | ')' | ']' | '}' | '"' | '”' | '»' | '…')
        || matches!(ch, '⋯')
        || matches!(ch, '\u{202F}' | '\u{00A0}' | '\u{2011}')
}

/// 片段 k 的行内容字节区间；行内容总长剥掉 `\r\n`。
fn fragment_byte_range(points: &[WrapPointInfo], k: usize, content_len: usize) -> Range<usize> {
    let start = k.checked_sub(1).map_or(0, |i| points[i].byte_ix);
    let end = points.get(k).map_or(content_len, |point| point.byte_ix);
    start..end
}

/// 目标字节所在片段的下标。片段为前闭后开区间，`byte == points[i].byte_ix`
/// 属于片段 i + 1。
fn fragment_index_for_byte(points: &[WrapPointInfo], byte: usize) -> usize {
    points
        .iter()
        .position(|point| point.byte_ix > byte)
        .unwrap_or(points.len())
}

fn push_isomorphic(transforms: &mut Vec<Transform>, lines: usize) {
    if let Some(last) = transforms.last_mut()
        && last.kind == TransformKind::Isomorphic
    {
        last.input_lines += lines;
        return;
    }
    transforms.push(Transform::isomorphic(lines));
}

/// 复制旧变换的一段输入行。Isomorphic item 可以合并很多行，不能直接用
/// SumTree cursor 在 item 中间切片，否则 Bias 会把整个 item 复制到结果中。
fn push_transform_slice(
    transforms: &mut Vec<Transform>,
    transform: &Transform,
    input_lines: usize,
) {
    if input_lines == 0 {
        return;
    }
    match transform.kind {
        TransformKind::Isomorphic => push_isomorphic(transforms, input_lines),
        TransformKind::Wrap => {
            debug_assert_eq!(input_lines, 1);
            transforms.push(transform.clone());
        }
    }
}

/// 相邻 tab 行号合并为不相交区间。
fn merge_ranges(rows: &[usize]) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for row in rows {
        match ranges.last_mut() {
            Some(last) if last.end >= *row => last.end = last.end.max(row + 1),
            _ => ranges.push(*row..row + 1),
        }
    }
    ranges
}

fn isomorphic_tree(tab_rows: usize) -> SumTree<Transform> {
    if tab_rows == 0 {
        SumTree::new(())
    } else {
        SumTree::from_item(Transform::isomorphic(tab_rows), ())
    }
}
