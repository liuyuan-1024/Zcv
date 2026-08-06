//! DisplayMap 的软换行（soft wrap）层。
//!
//! WrapMap 在 TabMap 之上，把超过指定像素宽度的逻辑行拆成多个显示行。
//! 换行点由 gpui 的 LineWrapper 计算（与 Zed 同源算法：词边界优先、长词硬断、首行缩进继承）。
//! 续行的视觉缩进是一段"假空格"，作为显示文本的前缀参与布局、命中测试与坐标换算，因此渲染端无需为续行做任何特殊定位。
//!
//! 与 FoldMap 一样，WrapMap 用 SumTree<Transform> 维护"输入 tab 行 → 输出显示行"的拓扑：Isomorphic 段把连续不换行行合并（含 fold 占位符行），Wrap 段把单个宽行拆成 `wrap_points.len() + 1` 个显示行。
//! 折叠与换行是正交的两层变换：折叠先塌缩文本，换行再按像素宽度切分。

use std::ops::Range;
use std::sync::Arc;

use gpui::{Font, LineFragment, LineWrapper, Pixels, TextSystem};
use gpui_sum_tree::{Bias, ContextLessSummary, Dimension, Dimensions, Item, SumTree};
use unicode_segmentation::UnicodeSegmentation;
use zcv_engine::{ByteOffset, CoordinateError, Line, LogicalColumn, Position, Snapshot, TextRange};

use super::display_width::DisplayColumn;

use super::error::DisplayMapResult;
use super::fold_map::StreamProjectedKind;
use super::fold_map::{
    FoldEdit, FoldPlaceholder, LogicalPoint, LogicalPointProjection, LogicalRange,
    ProjectedLineIndex, ProjectedPoint, ProjectedPointMapping, ProjectedRange,
};
use super::line_stream::StreamLineSource;
use super::tab_map::{TabSnapshot, advance_display_column, byte_for_display_column, line_content};
use super::{DisplayPoint, DisplayRow};

/// 换行点：行内容（已剥 `\r\n`）内的半开字节分界与下一续行的假空格数。
///
/// 显示行 i 的文本是 `content[prev_ix..ix]`，其中 `prev_ix` 是前一个换行点（首段为 0）；
/// 显示行 i + 1 以 `indent` 个假空格开头（与 gpui `Boundary` 一一对应）。
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
/// - Isomorphic：n 个 tab 行 → n 个显示行（连续不换行行合并；fold 占位符行必为此形态）；
/// - Wrap：1 个 tab 行 → `wrap_points.len() + 1` 个显示行。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Transform {
    kind: TransformKind,
    input_lines: usize,
    wrap_points: Vec<WrapPointInfo>,
}

impl Transform {
    fn isomorphic(input_lines: usize) -> Self {
        Self {
            kind: TransformKind::Isomorphic,
            input_lines,
            wrap_points: Vec::new(),
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
    /// 文本行（携带流来源：buffer 行或合成行）。
    Text(StreamLineSource),
    Placeholder(FoldPlaceholder),
}

/// 视口内单条显示行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapViewportRow<'a> {
    index: DisplayRow,
    kind: WrapViewportRowKind<'a>,
}

impl<'a> WrapViewportRow<'a> {
    pub fn new(index: DisplayRow, kind: WrapViewportRowKind<'a>) -> Self {
        Self { index, kind }
    }

    pub fn index(&self) -> DisplayRow {
        self.index
    }

    pub fn kind(&self) -> &WrapViewportRowKind<'a> {
        &self.kind
    }
}

/// 视口显示行内容种类。
///
/// Text 行携带整行投影文本（`text`，含行内提示注入，行尾换行未剥）与本段投影字节范围；
/// 渲染端在 `indent` > 0 时把假空格拼在段文本前面。
/// `source` 是行的文本来源（buffer 行 / 合成行，渲染端据此区分行号与可命中性；
/// 合成行无 buffer 坐标，`global_byte_start` 为锚定行行首）。
/// `column_base` 是该段起始的逻辑字符列，用于命中测试与选区列换算。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WrapViewportRowKind<'a> {
    Text {
        source: StreamLineSource,
        text: std::borrow::Cow<'a, str>,
        byte_range: Range<usize>,
        global_byte_start: usize,
        fragment_index: usize,
        indent: usize,
        column_base: usize,
    },
    Placeholder(FoldPlaceholder),
}

/// 一次显示行视口读取结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapViewportSlice<'a> {
    rows: Vec<WrapViewportRow<'a>>,
}

impl<'a> WrapViewportSlice<'a> {
    pub fn rows(&self) -> &[WrapViewportRow<'a>] {
        &self.rows
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

    pub(super) fn buffer_snapshot(&self) -> &Snapshot {
        self.tab_snapshot.buffer_snapshot()
    }

    #[cfg(test)]
    pub(super) const fn version(&self) -> u64 {
        self.version
    }

    pub(super) fn line_count(&self) -> usize {
        self.transforms.summary().output_rows
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
        self.logical_point_to_display_point(position.line(), position.column())
    }

    pub(super) fn display_point_to_offset(
        &self,
        point: DisplayPoint,
    ) -> DisplayMapResult<ByteOffset> {
        let fragment = self.display_row_to_fragment(point.row())?;
        match fragment.kind {
            WrapFragmentKind::Placeholder(_) => {
                // 现状语义：占位符行的任意位置都映射到 anchor 行首。
                let fold = self.tab_snapshot.fold_snapshot();
                let projected = ProjectedPoint::new(
                    ProjectedLineIndex::new(point.row().get()),
                    LogicalColumn::ZERO,
                );
                let logical = match fold.projected_to_logical_point(projected)? {
                    ProjectedPointMapping::Text(logical) => logical,
                    ProjectedPointMapping::Placeholder { anchor, .. } => anchor,
                };
                Ok(self
                    .tab_snapshot
                    .buffer_snapshot()
                    .position_to_byte(logical.into())?)
            }
            WrapFragmentKind::Text(source) => {
                let buffer = self.tab_snapshot.buffer_snapshot();
                let tab_row = Line::new(fragment.tab_row);
                // 合成行的字节范围是锚定行行首的伪坐标：映射到锚定行行首（与占位符同语义）。
                let line_start = self
                    .tab_snapshot
                    .line_byte_range(tab_row)
                    .ok_or(CoordinateError::LineOutOfBounds(tab_row))?
                    .start
                    .get();
                let StreamLineSource::Buffer(_) = source else {
                    return Ok(ByteOffset::new(line_start));
                };
                let text = self
                    .tab_snapshot
                    .line_text(tab_row)
                    .ok_or(CoordinateError::LineOutOfBounds(tab_row))?;
                let content = line_content(text.as_ref());
                let byte_range = fragment.byte_range;
                let local = byte_for_display_column(
                    &content[byte_range.clone()],
                    fragment.indent,
                    point.column().get(),
                    buffer,
                );
                // 投影行内偏移逆投影回原始行内偏移（注入段内吸附到锚定后）。
                let stream_line = self
                    .tab_snapshot
                    .stream_line_for_projected(tab_row)
                    .ok_or(CoordinateError::LineOutOfBounds(tab_row))?;
                let inlay = self.tab_snapshot.fold_snapshot().inlay_snapshot();
                let projected_byte = byte_range.start + local;
                let original_byte = inlay.to_original_offset(stream_line, projected_byte);
                Ok(ByteOffset::new(line_start + original_byte))
            }
        }
    }

    pub(super) fn slice_viewport(
        &self,
        start_row: DisplayRow,
        line_count: usize,
    ) -> DisplayMapResult<WrapViewportSlice<'_>> {
        let total = self.line_count();
        let start = start_row.get();
        if start > total {
            return Err(CoordinateError::LineOutOfBounds(Line::new(start)).into());
        }
        let end = start.saturating_add(line_count).min(total);
        let buffer = self.tab_snapshot.buffer_snapshot();
        let mut rows = Vec::with_capacity(end - start);
        for row in start..end {
            let fragment = self.display_row_to_fragment(DisplayRow::new(row))?;
            let kind = match fragment.kind {
                WrapFragmentKind::Placeholder(placeholder) => {
                    WrapViewportRowKind::Placeholder(placeholder)
                }
                WrapFragmentKind::Text(source) => {
                    // 投影文本（含行内提示注入；合成行无注入直接借用）；行首为原始 buffer 字节。
                    let tab_row = Line::new(fragment.tab_row);
                    let line_range = self
                        .tab_snapshot
                        .line_byte_range(tab_row)
                        .ok_or(CoordinateError::LineOutOfBounds(tab_row))?;
                    let text = self
                        .tab_snapshot
                        .line_text(tab_row)
                        .ok_or(CoordinateError::LineOutOfBounds(tab_row))?;
                    // 片段起点原始字节（投影偏移逆投影）→ 起始逻辑列。
                    let stream_line = self
                        .tab_snapshot
                        .stream_line_for_projected(tab_row)
                        .ok_or(CoordinateError::LineOutOfBounds(tab_row))?;
                    let inlay = self.tab_snapshot.fold_snapshot().inlay_snapshot();
                    let original_start =
                        inlay.to_original_offset(stream_line, fragment.byte_range.start);
                    let column_base = buffer
                        .byte_to_position(ByteOffset::new(line_range.start.get() + original_start))
                        .map_or(0, |position| position.column().get());
                    WrapViewportRowKind::Text {
                        source,
                        text,
                        byte_range: fragment.byte_range,
                        global_byte_start: line_range.start.get(),
                        fragment_index: fragment.fragment_index,
                        indent: fragment.indent,
                        column_base,
                    }
                }
            };
            rows.push(WrapViewportRow::new(DisplayRow::new(row), kind));
        }
        Ok(WrapViewportSlice { rows })
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
        let start = match fold.logical_to_projected_point(logical.start())? {
            LogicalPointProjection::Visible(point) => self.projected_point_to_range_point(point)?,
            LogicalPointProjection::Hidden {
                anchor_projected, ..
            } => (DisplayRow::from(anchor_projected.line()), 0),
        };
        let end = match fold.logical_to_projected_point(logical.end())? {
            LogicalPointProjection::Visible(point) => self.projected_point_to_range_point(point)?,
            LogicalPointProjection::Hidden {
                anchor_projected, ..
            } => (
                DisplayRow::new(
                    (anchor_projected.line().get() + 2).min(self.line_count().saturating_sub(1)),
                ),
                0,
            ),
        };
        if start.0 > end.0 || (start.0 == end.0 && start.1 > end.1) {
            return Ok(Vec::new());
        }

        // 文本行与折叠占位符使用不同的绘制路径。跨过二者边界的范围必须拆成
        // 最大连续段，否则 selection 会把占位符误当成普通文本行。
        let mut breakpoints = vec![start];
        let mut previous_is_placeholder = self.display_row_is_placeholder(start.0);
        for row in (start.0.get() + 1)..=end.0.get() {
            let is_placeholder = self.display_row_is_placeholder(DisplayRow::new(row));
            if is_placeholder != previous_is_placeholder {
                breakpoints.push((DisplayRow::new(row), 0));
                previous_is_placeholder = is_placeholder;
            }
        }
        breakpoints.push(end);

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
            WrapFragmentKind::Text(source) => {
                let tab_row = Line::new(fragment.tab_row);
                // 合成行的字节范围是锚定行行首的伪坐标：映射到锚定行行首。
                let line_start = self
                    .tab_snapshot
                    .line_byte_range(tab_row)
                    .ok_or(CoordinateError::LineOutOfBounds(tab_row))?
                    .start
                    .get();
                let StreamLineSource::Buffer(_) = source else {
                    return Ok(ByteOffset::new(line_start));
                };
                // 片段终点（投影偏移）逆投影回原始行内偏移。
                let stream_line = self
                    .tab_snapshot
                    .stream_line_for_projected(tab_row)
                    .ok_or(CoordinateError::LineOutOfBounds(tab_row))?;
                let inlay = self.tab_snapshot.fold_snapshot().inlay_snapshot();
                let original_end = inlay.to_original_offset(stream_line, fragment.byte_range.end);
                Ok(ByteOffset::new(line_start + original_end))
            }
            WrapFragmentKind::Placeholder(placeholder) => {
                let buffer = self.tab_snapshot.buffer_snapshot();
                let anchor = placeholder.anchor_line();
                let line_slice = buffer.slice_line(anchor)?;
                let content = line_content(line_slice.as_str());
                Ok(ByteOffset::new(
                    buffer.line_start_byte(anchor)?.get() + content.len(),
                ))
            }
        }
    }

    /// 显示行 → (tab 行, 片段信息)。
    fn display_row_to_fragment(&self, row: DisplayRow) -> DisplayMapResult<WrapFragment> {
        let (start, _, transform) =
            self.transforms
                .find::<OutputToInput, _>((), &OutputRows(row.get()), Bias::Right);
        let transform =
            transform.ok_or(CoordinateError::LineOutOfBounds(Line::new(row.get())))?;
        let input_start = start.1.0;
        let output_start = start.0.0;
        match transform.kind {
            TransformKind::Isomorphic => {
                let tab_row = input_start + (row.get() - output_start);
                let kind = self.projected_kind(tab_row)?;
                let content_len = match kind {
                    WrapFragmentKind::Placeholder(_) => 0,
                    _ => line_content(
                        self.tab_snapshot
                            .line_text(Line::new(tab_row))
                            .expect("可见行必须可解析")
                            .as_ref(),
                    )
                    .len(),
                };
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
                let content_len = match kind {
                    WrapFragmentKind::Placeholder(_) => 0,
                    _ => line_content(
                        self.tab_snapshot
                            .line_text(Line::new(input_start))
                            .expect("可见行必须可解析")
                            .as_ref(),
                    )
                    .len(),
                };
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
            Some(kind) => Ok(match kind {
                StreamProjectedKind::Text(source) => WrapFragmentKind::Text(source),
                StreamProjectedKind::Placeholder(placeholder) => {
                    WrapFragmentKind::Placeholder(placeholder)
                }
            }),
            None => Err(CoordinateError::LineOutOfBounds(Line::new(tab_row)).into()),
        }
    }

    fn display_row_is_placeholder(&self, row: DisplayRow) -> bool {
        self.display_row_to_fragment(row)
            .is_ok_and(|fragment| matches!(fragment.kind, WrapFragmentKind::Placeholder(_)))
    }

    /// 逻辑行内的点 → 显示点；列 = 显示行内 display column（含假空格缩进）。
    fn logical_point_to_display_point(
        &self,
        line: Line,
        column: LogicalColumn,
    ) -> DisplayMapResult<DisplayPoint> {
        let fold = self.tab_snapshot.fold_snapshot();
        match fold.logical_to_projected_point(LogicalPoint::new(line, column))? {
            LogicalPointProjection::Visible(point) => self.projected_point_to_display_point(point),
            LogicalPointProjection::Hidden {
                anchor_projected, ..
            } => Ok(DisplayPoint::new(
                DisplayRow::from(anchor_projected.line()),
                DisplayColumn::ZERO,
            )),
        }
    }

    /// 投影点（tab 行 + 逻辑列）→ 显示点。
    fn projected_point_to_display_point(
        &self,
        point: ProjectedPoint,
    ) -> DisplayMapResult<DisplayPoint> {
        let tab_row = point.line().get();
        let line = Line::new(tab_row);
        let buffer = self.tab_snapshot.buffer_snapshot();
        // 投影文本（含行内提示注入）；目标列对应的原始字节经注入前缀映射为投影偏移。
        let text = self
            .tab_snapshot
            .line_text(line)
            .ok_or(CoordinateError::LineOutOfBounds(line))?;
        let content = line_content(text.as_ref());
        let line_start = self
            .tab_snapshot
            .line_byte_range(line)
            .ok_or(CoordinateError::LineOutOfBounds(line))?
            .start
            .get();
        // 逻辑列 → 原始行内字节：position_to_byte 需要 buffer 行号（投影行号经流解析回 buffer 行）。
        let stream_line = self
            .tab_snapshot
            .stream_line_for_projected(line)
            .ok_or(CoordinateError::LineOutOfBounds(line))?;
        let inlay = self.tab_snapshot.fold_snapshot().inlay_snapshot();
        let buffer_line = match inlay.source(stream_line) {
            Some(StreamLineSource::Buffer(buffer_line)) => Line::new(buffer_line),
            _ => return Err(CoordinateError::LineOutOfBounds(line).into()),
        };
        let target_byte = buffer
            .position_to_byte(Position::new(buffer_line, point.column()))?
            .get()
            - line_start;
        let target_projected = inlay.to_projected_offset(stream_line, target_byte);
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
                advance_display_column(column, grapheme, buffer)
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
        let transform =
            transform.ok_or(CoordinateError::LineOutOfBounds(Line::new(tab_row)))?;
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
        let line_start = self
            .tab_snapshot
            .line_byte_range(line)
            .ok_or(CoordinateError::LineOutOfBounds(line))?
            .start
            .get();
        // 逻辑列 → 原始行内字节：position_to_byte 需要 buffer 行号（投影行号经流解析回 buffer 行）。
        let stream_line = self
            .tab_snapshot
            .stream_line_for_projected(line)
            .ok_or(CoordinateError::LineOutOfBounds(line))?;
        let inlay = self.tab_snapshot.fold_snapshot().inlay_snapshot();
        let buffer_line = match inlay.source(stream_line) {
            Some(StreamLineSource::Buffer(buffer_line)) => Line::new(buffer_line),
            _ => return Err(CoordinateError::LineOutOfBounds(line).into()),
        };
        let target_byte = buffer
            .position_to_byte(Position::new(buffer_line, point.column()))?
            .get()
            - line_start;
        let target_projected = inlay.to_projected_offset(stream_line, target_byte);
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
        // 片段起点逆投影回原始字节 → 起始逻辑列。
        let original_start = inlay.to_original_offset(stream_line, fragment_start);
        let column_base = buffer
            .byte_to_position(ByteOffset::new(line_start + original_start))
            .map_or(0, |position| position.column().get());
        Ok((
            DisplayRow::new(output_start + fragment_index),
            indent + (point.column().get() - column_base),
        ))
    }
}

#[derive(Clone)]
pub(super) struct WrapMap {
    snapshot: WrapSnapshot,
    wrap_width: Option<Pixels>,
    font_with_size: Option<(Font, Pixels)>,
    /// 由 `set_wrap_width` 缓存；重排时创建 LineWrapper 用（LineWrapper 只能从
    /// text system 的池里获取，而 sync 发生在 buffer 编辑时拿不到 window）。
    text_system: Option<Arc<TextSystem>>,
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
            },
            snapshot,
        )
    }

    pub(super) fn snapshot(&self) -> &WrapSnapshot {
        &self.snapshot
    }

    /// 同步 tab 层变化。tab 版本未变时不做任何事；换行开启时按 fold edit 的
    /// 结构性变化全量重排或按 changed_lines 增量重排，关闭时重建单段透传。
    pub(super) fn sync(&mut self, tab_snapshot: TabSnapshot, fold_edits: &[FoldEdit]) {
        if tab_snapshot.version() == self.snapshot.tab_snapshot.version() {
            return;
        }
        self.snapshot.tab_snapshot = tab_snapshot;
        if let (Some(wrap_width), Some((font, font_size))) =
            (self.wrap_width, self.font_with_size.as_ref())
        {
            let text_system = self
                .text_system
                .as_ref()
                .expect("换行开启时必须先通过 set_wrap_width 缓存 text system");
            let mut wrapper = text_system.line_wrapper(font.clone(), *font_size);
            // 合成行/行内提示变化由 fold 发 structural edit（单一信号）。
            if fold_edits.iter().any(FoldEdit::is_structural) {
                self.rewrap_all(wrap_width, &mut wrapper);
            } else {
                let changed_lines: Vec<Line> = fold_edits
                    .iter()
                    .flat_map(|edit| edit.changed_lines().iter().copied())
                    .collect();
                self.update_inline(&changed_lines, wrap_width, &mut wrapper);
            }
        } else {
            self.set_isomorphic_all();
        }
        self.check_invariants();
        self.snapshot.version += 1;
    }

    /// 设置换行宽度与字体。只有 (宽度, 字体, 字号) 任一变化时才重建；
    /// 返回是否发生了变化。
    pub(super) fn set_wrap_width(
        &mut self,
        wrap_width: Option<Pixels>,
        font: Font,
        font_size: Pixels,
        text_system: Arc<TextSystem>,
    ) -> bool {
        let width_changed = wrap_width != self.wrap_width;
        let font_changed =
            self.font_with_size
                .as_ref()
                .is_some_and(|(cached_font, cached_size)| {
                    *cached_font != font || *cached_size != font_size
                });
        let needs_rewrap = width_changed || (font_changed && wrap_width.is_some());
        self.text_system = Some(text_system);
        if !needs_rewrap {
            return false;
        }
        match wrap_width {
            None => self.set_isomorphic_all(),
            Some(width) => {
                let text_system = self
                    .text_system
                    .as_ref()
                    .expect("设置换行宽度时必须提供 text system");
                let mut wrapper = text_system.line_wrapper(font.clone(), font_size);
                self.rewrap_all(width, &mut wrapper);
            }
        }
        self.wrap_width = wrap_width;
        self.font_with_size = Some((font, font_size));
        self.snapshot.version += 1;
        true
    }

    fn set_isomorphic_all(&mut self) {
        self.snapshot.transforms = isomorphic_tree(self.snapshot.tab_snapshot.line_count());
        self.snapshot.wrapped = false;
        self.check_invariants();
    }

    /// 全量重建：对每个 tab 行重新计算换行点。
    fn rewrap_all(&mut self, wrap_width: Pixels, wrapper: &mut LineWrapper) {
        let mut transforms = Vec::new();
        for tab_row in 0..self.snapshot.tab_snapshot.line_count() {
            self.push_wrap_transform(&mut transforms, tab_row, wrap_width, wrapper);
        }
        self.snapshot.transforms = SumTree::from_iter(transforms, ());
        self.snapshot.wrapped = true;
        self.check_invariants();
    }

    /// 行级增量：只重排 changed_lines 对应的 tab 行，其余段落原样保留。
    fn update_inline(
        &mut self,
        changed_lines: &[Line],
        wrap_width: Pixels,
        wrapper: &mut LineWrapper,
    ) {
        let fold = self.snapshot.tab_snapshot.fold_snapshot();
        let mut rows: Vec<usize> = changed_lines
            .iter()
            .filter_map(|line| match fold.logical_to_projected(*line).ok()? {
                super::fold_map::LogicalProjection::Visible(row) => Some(row.get()),
                super::fold_map::LogicalProjection::Hidden { .. } => None,
            })
            .collect();
        rows.sort_unstable();
        rows.dedup();
        if rows.is_empty() {
            return;
        }
        let row_edits = merge_ranges(&rows);
        let old_transforms: Vec<_> = self.snapshot.transforms.iter().cloned().collect();
        let mut new_transforms = Vec::new();
        let mut edit_index = 0;
        let mut input_start = 0;
        for transform in &old_transforms {
            let input_end = input_start + transform.input_lines;
            while row_edits
                .get(edit_index)
                .is_some_and(|edit| edit.end <= input_start)
            {
                edit_index += 1;
            }

            let mut position = input_start;
            while let Some(edit) = row_edits.get(edit_index) {
                if edit.start >= input_end {
                    break;
                }
                let unchanged_end = edit.start.max(position).min(input_end);
                push_transform_slice(&mut new_transforms, transform, unchanged_end - position);

                let changed_start = edit.start.max(position);
                let changed_end = edit.end.min(input_end);
                for tab_row in changed_start..changed_end {
                    self.push_wrap_transform(&mut new_transforms, tab_row, wrap_width, wrapper);
                }
                position = changed_end;
                if edit.end <= input_end {
                    edit_index += 1;
                } else {
                    break;
                }
            }
            push_transform_slice(&mut new_transforms, transform, input_end - position);
            input_start = input_end;
        }
        self.snapshot.transforms = SumTree::from_iter(new_transforms, ());
        self.snapshot.wrapped = true;
        self.check_invariants();
    }

    /// 计算单个 tab 行的换行变换并压入（相邻 Isomorphic 自动合并）。
    fn push_wrap_transform(
        &self,
        transforms: &mut Vec<Transform>,
        tab_row: usize,
        wrap_width: Pixels,
        wrapper: &mut LineWrapper,
    ) {
        match self.snapshot.projected_kind(tab_row) {
            Err(_) => push_isomorphic(transforms, 1),
            Ok(WrapFragmentKind::Placeholder(_)) => push_isomorphic(transforms, 1),
            Ok(WrapFragmentKind::Text(_)) => {
                // 文本统一走流：buffer 行与合成行（外部文本）共用换行计算，软换行免费。
                let text = self
                    .snapshot
                    .tab_snapshot
                    .line_text(Line::new(tab_row))
                    .expect("可见行必须可解析");
                let content = line_content(text.as_ref());
                let boundaries: Vec<_> = wrapper
                    .wrap_line(&[LineFragment::text(content)], wrap_width)
                    .collect();
                if boundaries.is_empty() {
                    push_isomorphic(transforms, 1);
                } else {
                    transforms.push(Transform {
                        kind: TransformKind::Wrap,
                        input_lines: 1,
                        wrap_points: boundaries
                            .iter()
                            .map(|boundary| WrapPointInfo {
                                byte_ix: boundary.ix,
                                indent: boundary.next_indent,
                            })
                            .collect(),
                    });
                }
            }
        }
    }

    #[cfg(debug_assertions)]
    fn check_invariants(&self) {
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

/// 片段 k 的行内容字节区间；`content_len` 是行内容总长（剥 `\r\n`）。
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
