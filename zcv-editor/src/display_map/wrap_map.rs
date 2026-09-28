//! DisplayMap 的软换行（soft wrap）层。
//!
//! WrapMap 在 TabMap 之上，把超过指定像素宽度的逻辑行拆成多个显示行。
//! 换行点由文本系统对展开后的整行文本进行 shaping，再按词边界优先、长词硬断和首行缩进继承规则计算。
//! 续行的视觉缩进是一段"假空格"，作为显示文本的前缀参与布局、命中测试与坐标换算，因此渲染端无需为续行做任何特殊定位。
//!
//! 与 FoldMap 一样，WrapMap 用 `SumTree<Transform>` 维护"输入 tab 行 → 输出显示行"的拓扑：Isomorphic 段把连续不换行行合并，Wrap 段把单个宽行拆成 `wrap_points.len() + 1` 个显示行。
//! 折叠与换行是正交的两层变换：折叠先塌缩文本，换行再按像素宽度切分。

use zcv_multi_buffer::{MultiBufferOffset, MultiBufferRange};

use std::borrow::Cow;
use std::collections::VecDeque;
use std::mem;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use futures_lite::future::yield_now;
use gpui::{AppContext as _, Context, Font, Pixels, Task, TextRun, TextSystem, WindowTextSystem};
use sum_tree::{Bias, ContextLessSummary, Dimension, Dimensions, Item, SumTree};
use unicode_segmentation::UnicodeSegmentation;
use zcv_multi_buffer::{ExcerptSnapshot, MultiBufferLineCursor, MultiBufferSnapshot};
use zcv_text::{CoordinateError, Line, LogicalColumn};

use super::chunk::{Chunk, ChunkText, FoldChunks, HighlightStyles, StyledChunks};
use super::display_width::DisplayColumn;
use super::error::DisplayMapResult;
use super::fold_map::{
    ChunkRendererId, FoldBias, FoldOffset, FoldRowSegment, FoldRowSegmentKind, FoldRows,
    LogicalPoint, LogicalRange, ProjectedLineIndex, ProjectedPoint, ProjectedRange,
    StreamProjectedKind,
};
use super::tab_map::{
    TabEdit, TabPoint, TabPointMapping, TabSnapshot, advance_display_column, byte_after_chars,
    byte_for_display_column, display_width_for_fold_row, line_content,
};
use super::{WrapPoint, WrapRow};

const WRAP_YIELD_ROW_INTERVAL: usize = 100;
const FULL_REWRAP_BUDGET: Duration = Duration::from_millis(5);
const INCREMENTAL_REWRAP_BUDGET: Duration = Duration::from_millis(1);

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
    /// 变换树的输入维度是 Tab 点，不是独立的行计数。
    ///
    /// 当前 Wrap 以完整逻辑行为最小重排单元，所以同构段的列为 0；
    /// 精确行内端点仍由 `TabEdit` 保留，并在重排边界扩展为完整行。
    input: TabPoint,
    output_rows: usize,
    /// 本变换输出区间内最长行的相对行号与显示宽度字符数。
    ///
    /// 只在透传（未换行）变换树上维护，作为 `WrapSnapshot` summary 的派生维度；
    /// 查询端按 O(1) 读取，不按帧扫描全部行。
    longest_row: usize,
    longest_row_chars: usize,
    /// 换行点共享存储：克隆 Transform（增量重建时大量发生）只增加引用计数，不深拷贝换行点。
    wrap_points: Arc<[WrapPointInfo]>,
}

impl Transform {
    fn isomorphic(input: TabPoint, output_rows: usize) -> Self {
        Self {
            kind: TransformKind::Isomorphic,
            input,
            output_rows,
            longest_row: 0,
            longest_row_chars: 0,
            wrap_points: Vec::new().into(),
        }
    }

    fn output_rows(&self) -> usize {
        match self.kind {
            TransformKind::Isomorphic => self.output_rows,
            TransformKind::Wrap => self.output_rows,
        }
    }
}

impl Item for Transform {
    type Summary = TransformSummary;

    fn summary(&self, (): ()) -> Self::Summary {
        TransformSummary {
            input: self.input,
            output_rows: self.output_rows(),
            longest_row: self.longest_row,
            longest_row_chars: self.longest_row_chars,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TransformSummary {
    input: TabPoint,
    output_rows: usize,
    /// 输出区间内最长行的相对行号与字符数；对齐 Zed `TextSummary` 的 `longest_row` 维度。
    longest_row: usize,
    longest_row_chars: usize,
}

impl ContextLessSummary for TransformSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        let output_rows_before = self.output_rows;
        self.input = self.input.advance(summary.input);
        self.output_rows += summary.output_rows;
        if summary.longest_row_chars > self.longest_row_chars {
            self.longest_row = output_rows_before + summary.longest_row;
            self.longest_row_chars = summary.longest_row_chars;
        }
    }
}

impl<'a> Dimension<'a, TransformSummary> for TabPoint {
    fn zero((): ()) -> Self {
        Self::zero()
    }

    fn add_summary(&mut self, summary: &'a TransformSummary, (): ()) {
        *self = self.advance(summary.input);
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

type InputToOutput = Dimensions<TabPoint, OutputRows>;
type OutputToInput = Dimensions<OutputRows, TabPoint>;

/// 显示行对应的行片段信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WrapFragment {
    pub(super) tab_row: usize,
    pub(super) kind: WrapFragmentKind,
    /// 行内容（已剥 `\r\n`）内的半开字节区间。
    pub(super) byte_range: Range<usize>,
    /// 锚点行在组合文档中的内容字节范围（合并行为入口行内容）。
    pub(super) content_range: Range<MultiBufferOffset>,
    /// 该显示行开头的假空格数（逻辑行首显示行为 0）。
    pub(super) indent: usize,
    /// 该显示行在所属逻辑行内的序号（0 = 逻辑行首显示行，gutter 行号在此）。
    pub(super) fragment_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WrapFragmentKind {
    /// 文本行（携带对应的 buffer 行来源）。
    Text(Line),
}

/// Wrap 层输出的行元数据。
///
/// 文本本身由下游 `FoldSnapshot`/`TabSnapshot` 在消费 chunk 时按投影行读取；
/// 游标只传递坐标和来源，不在每一行物化 `Cow`/`Arc` 文本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WrapRowKind {
    Text {
        source: Line,
        projected_line: usize,
        byte_range: Range<usize>,
        /// 锚点行在组合文档中的内容字节范围；下游按它读取行文本，不再逐行回查组合坐标。
        content_range: Range<MultiBufferOffset>,
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
    /// 逐行推进的组合行内容游标；一次定位后不再逐行对映射树整树 seek。
    line_cursor: Option<MultiBufferLineCursor<'a>>,
    /// Fold 行游标把投影行连续映射回组合文档行。
    fold_rows: FoldRows<'a>,
    /// 同一逻辑行的软换行片段共享组合范围和 shaping 长度。
    current_tab_row_content: Option<(usize, Range<MultiBufferOffset>, usize)>,
}

impl<'a> WrapRows<'a> {
    pub(super) fn new(snapshot: &'a WrapSnapshot, start: usize, end: usize) -> Self {
        let end = end.min(snapshot.transforms.summary().output_rows);
        let mut cursor = snapshot.transforms.cursor::<OutputToInput>(());
        cursor.seek(&OutputRows(start), Bias::Right);
        let tab_row = cursor.start().1.row();
        let fold_rows = snapshot.tab_snapshot.fold_snapshot().rows(tab_row);
        Self {
            snapshot,
            cursor,
            row: start,
            end,
            line_cursor: None,
            fold_rows,
            current_tab_row_content: None,
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
            .fragment_in_transform(
                transform,
                transform_start,
                self.row,
                &mut self.fold_rows,
                &mut self.line_cursor,
                &mut self.current_tab_row_content,
            )
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

    /// 当前文本行所属的 excerpt；沿用行内容游标，避免为每个可见行重新定位组合树。
    pub(super) fn current_excerpt(&self) -> Option<ExcerptSnapshot> {
        self.line_cursor
            .as_ref()
            .and_then(MultiBufferLineCursor::excerpt_snapshot)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WrapSnapshot {
    tab_snapshot: TabSnapshot,
    transforms: SumTree<Transform>,
    /// 是否处于软换行模式（false = 透传，显示行 == tab 行）。
    wrapped: bool,
    /// 是否只是按编辑急切插值、尚未经过真实 shaping 重排（由后台任务补齐）。
    interpolated: bool,
    version: u64,
}

impl WrapSnapshot {
    fn check_invariants(&self) {
        #[cfg(debug_assertions)]
        {
            let tab_rows = self.tab_snapshot.line_count();
            assert_eq!(
                self.transforms.summary().input,
                TabPoint::new(tab_rows, 0),
                "Wrap 输入点必须由当前 Tab 快照的投影边界确定"
            );
            for transform in self.transforms.iter() {
                match transform.kind {
                    TransformKind::Isomorphic => assert!(transform.input.row() > 0),
                    TransformKind::Wrap => {
                        assert_eq!(transform.input, TabPoint::new(1, 0));
                        assert!(!transform.wrap_points.is_empty());
                    }
                }
            }
        }
    }

    pub(crate) fn tab_snapshot(&self) -> &TabSnapshot {
        &self.tab_snapshot
    }

    pub(super) fn point_cursor(&self) -> WrapPointCursor<'_> {
        WrapPointCursor {
            snapshot: self,
            cursor: self.transforms.cursor::<InputToOutput>(()),
            tab_row: None,
            line_text: None,
        }
    }

    pub(super) fn buffer_snapshot(&self) -> &MultiBufferSnapshot {
        self.tab_snapshot.buffer_snapshot()
    }

    pub(super) fn line_count(&self) -> usize {
        self.transforms.summary().output_rows
    }

    /// 透传（未换行）模式下的最长行（wrap 行号）。
    ///
    /// 值来自变换树 summary，查询 O(1)；软换行模式下该维度不维护，调用方必须先确认未换行。
    pub(super) fn longest_row(&self) -> usize {
        debug_assert!(!self.wrapped, "最长行 summary 只在透传（未换行）模式下维护");
        self.transforms.summary().longest_row
    }

    /// 按编辑急切插值：结构编辑区间用 isomorphic 段占位，不重新 shaping。
    ///
    /// 后台重排完成前，变换树仍与新的 tab 行数保持一致；interpolated 标记它尚未真实测量。
    fn interpolate(
        &mut self,
        new_tab_snapshot: TabSnapshot,
        tab_edits: &[TabEdit],
    ) -> Vec<WrapEdit> {
        let structural = tab_edit_rows(tab_edits);
        if structural.is_empty() {
            debug_assert_eq!(
                self.transforms.summary().input.row(),
                new_tab_snapshot.line_count(),
                "Tab 行数变化却没有结构编辑；下层投影链行覆盖不一致"
            );
            self.tab_snapshot = new_tab_snapshot;
            self.interpolated = true;
            self.version += 1;
            return Vec::new();
        }
        let measure = self.transforms.clone();
        let old_transforms = mem::replace(&mut self.transforms, SumTree::new(()));
        let mut cursor = old_transforms.cursor::<TabPoint>(());
        let mut new_tree = SumTree::new(());
        let mut buffered = Vec::new();
        let mut old_ranges = Vec::with_capacity(structural.len());
        let mut measured = Vec::with_capacity(structural.len());

        let mut edits = structural.iter().peekable();
        if let Some((old_rows, _)) = edits.peek() {
            new_tree.append(
                cursor.slice(&TabPoint::new(old_rows.start, 0), Bias::Right),
                (),
            );
        }
        while let Some((old_rows, new_rows)) = edits.next() {
            // 用新快照补齐「已发出新行 → 编辑新起点」的保留行；
            // 这段只能是旧同构变换内尚未覆盖的行，按同构占位。
            let gap = new_rows
                .start
                .saturating_sub(new_tree.summary().input.row());
            if gap > 0 {
                push_isomorphic(&mut buffered, gap);
            }
            // 急切插值：新行按同构占位，等待后台真实重排。
            push_isomorphic(&mut buffered, new_rows.len());
            old_ranges.push(old_rows.clone());
            measured.push(new_rows.len());
            new_tree.extend(buffered.drain(..), ());

            // 旧游标只向前推进到编辑终点，不越过包含它的旧变换。
            cursor.seek_forward(&TabPoint::new(old_rows.end, 0), Bias::Right);
            let trailing = if let Some((next_old, _)) = edits.peek() {
                if next_old.start > cursor.end().row() {
                    // 当前旧变换整体落在两编辑之间：尾部以同构占位，随后搬运整段旧变换。
                    if cursor.end().row() > old_rows.end {
                        push_isomorphic(&mut buffered, cursor.end().row() - old_rows.end);
                        new_tree.extend(buffered.drain(..), ());
                    }
                    cursor.next();
                    Some(cursor.slice(&TabPoint::new(next_old.start, 0), Bias::Right))
                } else {
                    // 下一编辑仍在当前旧变换内：其间的同构行由下一轮 gap 补齐。
                    None
                }
            } else {
                if cursor.end().row() > old_rows.end {
                    push_isomorphic(&mut buffered, cursor.end().row() - old_rows.end);
                    new_tree.extend(buffered.drain(..), ());
                }
                cursor.next();
                Some(cursor.suffix())
            };
            if let Some(trailing) = trailing {
                new_tree.append(trailing, ());
            }
        }
        debug_assert_eq!(
            new_tree.summary().input.row(),
            new_tab_snapshot.line_count(),
            "Wrap 急切插值覆盖不匹配；row_edits={structural:?}"
        );
        self.transforms = new_tree;
        self.tab_snapshot = new_tab_snapshot;
        self.wrapped = true;
        self.interpolated = true;
        self.version += 1;
        wrap_edits(&measure, &old_ranges, &measured)
    }

    pub(super) fn is_wrapped(&self) -> bool {
        self.wrapped
    }

    pub(super) fn offset_to_wrap_point(
        &self,
        offset: MultiBufferOffset,
    ) -> DisplayMapResult<WrapPoint> {
        let position = self
            .tab_snapshot
            .buffer_snapshot()
            .byte_to_position(offset)?;
        // fold 拓扑的输入坐标是流行号。
        let stream_line = position.line();
        self.logical_point_to_wrap_point(stream_line, position.column())
    }

    pub(super) fn wrap_point_to_offset(
        &self,
        point: WrapPoint,
    ) -> DisplayMapResult<MultiBufferOffset> {
        self.wrap_point_to_offset_with_bias(point, FoldBias::Left)
    }

    pub(super) fn wrap_point_to_offset_with_bias(
        &self,
        point: WrapPoint,
        bias: FoldBias,
    ) -> DisplayMapResult<MultiBufferOffset> {
        let fragment = self.wrap_row_to_fragment(point.row())?;
        match fragment.kind {
            WrapFragmentKind::Text(_source) => {
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
                        self.tab_snapshot().tab_width().get(),
                    );
                    return self.merged_byte_to_offset(
                        fragment.tab_row,
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
                    self.tab_snapshot().tab_width().get(),
                );
                let projected_byte = byte_range.start + local;
                Ok(MultiBufferOffset::new(line_start + projected_byte))
            }
        }
    }

    /// 折叠合并行内合并字节 → buffer 字节。
    ///
    /// 文本段按行内逆投影映射；
    /// 占位符列按选区方向吸附到折叠输入起点或终点，这样拖拽经过折叠时隐藏内容会整体纳入选区。
    fn merged_byte_to_offset(
        &self,
        tab_row: usize,
        segments: &[FoldRowSegment],
        merged_byte: usize,
        bias: FoldBias,
    ) -> DisplayMapResult<MultiBufferOffset> {
        let fold = self.tab_snapshot.fold_snapshot();
        let row_start = fold
            .row_start_offset(ProjectedLineIndex::new(tab_row))
            .get();
        for (index, segment) in segments.iter().enumerate() {
            let at_last = index + 1 == segments.len();
            if merged_byte >= segment.merged_range.end && !at_last {
                continue;
            }
            return match &segment.kind {
                FoldRowSegmentKind::Text {
                    stream_line,
                    projected_range,
                } => {
                    let local = merged_byte
                        .saturating_sub(segment.merged_range.start)
                        .min(segment.merged_range.len());
                    self.stream_offset(*stream_line, projected_range.start + local)
                }
                FoldRowSegmentKind::Placeholder { .. } => {
                    let output = FoldOffset::new(MultiBufferOffset::new(
                        row_start + segment.merged_range.start,
                    ));
                    let (start, end) = fold.input_range_at_output(output);
                    match bias {
                        FoldBias::Left => Ok(start),
                        FoldBias::Right => Ok(end),
                    }
                }
            };
        }
        Err(CoordinateError::LineOutOfBounds(Line::new(tab_row)).into())
    }

    fn stream_offset(
        &self,
        stream_line: Line,
        original: usize,
    ) -> DisplayMapResult<MultiBufferOffset> {
        let range = self
            .tab_snapshot
            .fold_snapshot()
            .buffer_snapshot()
            .line_byte_range(stream_line)
            .ok_or(CoordinateError::LineOutOfBounds(stream_line))?;
        Ok(MultiBufferOffset::new(range.start.get() + original))
    }

    pub(super) fn rows(&self, start: usize, end: usize) -> WrapRows<'_> {
        WrapRows::new(self, start, end.min(self.line_count()))
    }

    /// 折叠合并行的 shaping 行长度（各段之和）；非合并行返回 None。
    fn fold_row_len(&self, tab_row: usize) -> Option<usize> {
        self.tab_snapshot
            .fold_snapshot()
            .fold_row_segments(ProjectedLineIndex::new(tab_row))
            .map(|segments| {
                segments
                    .last()
                    .expect("折叠合并行必须至少包含一个段")
                    .merged_range()
                    .end
            })
    }

    /// 锚点行的内容范围与 shaping 行长度；行内容经前向游标推进，不逐行整树 seek。
    fn cursor_tab_row_content<'a>(
        &'a self,
        tab_row: usize,
        stream_line: Line,
        line_cursor: &mut Option<MultiBufferLineCursor<'a>>,
    ) -> DisplayMapResult<(Range<MultiBufferOffset>, usize)> {
        let line = Line::new(tab_row);
        let fold = self.tab_snapshot.fold_snapshot();
        if line_cursor.is_none() {
            *line_cursor = MultiBufferLineCursor::new(fold.buffer_snapshot(), stream_line);
        }
        let cursor = line_cursor
            .as_mut()
            .ok_or(CoordinateError::LineOutOfBounds(line))?;
        if !cursor.seek(stream_line) {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        }
        let (start, len) = cursor
            .line_content_range()
            .ok_or(CoordinateError::LineOutOfBounds(line))?;
        let content_range = MultiBufferOffset::new(start)..MultiBufferOffset::new(start + len);
        Ok((content_range, self.fold_row_len(tab_row).unwrap_or(len)))
    }

    /// 无状态版本：命令路径按单行点查询求锚点行内容范围。
    fn tab_row_content(
        &self,
        tab_row: usize,
    ) -> DisplayMapResult<(Range<MultiBufferOffset>, usize)> {
        let line = Line::new(tab_row);
        let fold = self.tab_snapshot.fold_snapshot();
        let stream_line = self
            .tab_snapshot
            .stream_line_for_projected(line)
            .ok_or(CoordinateError::LineOutOfBounds(line))?;
        let content_range = fold
            .buffer_snapshot()
            .line_content_byte_range(stream_line)
            .ok_or(CoordinateError::LineOutOfBounds(line))?;
        let len = content_range.end.get() - content_range.start.get();
        Ok((content_range, self.fold_row_len(tab_row).unwrap_or(len)))
    }

    fn fragment_in_transform<'a>(
        &'a self,
        transform: &Transform,
        transform_start: OutputToInput,
        row: usize,
        fold_rows: &mut FoldRows<'a>,
        line_cursor: &mut Option<MultiBufferLineCursor<'a>>,
        current_tab_row_content: &mut Option<(usize, Range<MultiBufferOffset>, usize)>,
    ) -> DisplayMapResult<WrapFragment> {
        let output_start = transform_start.0.0;
        let input_start = transform_start.1.row();
        match transform.kind {
            TransformKind::Isomorphic => {
                let tab_row = input_start + row - output_start;
                let (kind, content_range, content_len) = self.cursor_tab_row_content_cached(
                    tab_row,
                    fold_rows,
                    line_cursor,
                    current_tab_row_content,
                )?;
                Ok(WrapFragment {
                    tab_row,
                    kind,
                    byte_range: 0..content_len,
                    content_range,
                    indent: 0,
                    fragment_index: 0,
                })
            }
            TransformKind::Wrap => {
                let (kind, content_range, content_len) = self.cursor_tab_row_content_cached(
                    input_start,
                    fold_rows,
                    line_cursor,
                    current_tab_row_content,
                )?;
                let fragment_index = row - output_start;
                Ok(WrapFragment {
                    tab_row: input_start,
                    kind,
                    byte_range: fragment_byte_range(
                        &transform.wrap_points,
                        fragment_index,
                        content_len,
                    ),
                    content_range,
                    indent: fragment_index
                        .checked_sub(1)
                        .map_or(0, |index| transform.wrap_points[index].indent as usize),
                    fragment_index,
                })
            }
        }
    }

    fn cursor_tab_row_content_cached<'a>(
        &'a self,
        tab_row: usize,
        fold_rows: &mut FoldRows<'a>,
        line_cursor: &mut Option<MultiBufferLineCursor<'a>>,
        cached: &mut Option<(usize, Range<MultiBufferOffset>, usize)>,
    ) -> DisplayMapResult<(WrapFragmentKind, Range<MultiBufferOffset>, usize)> {
        if let Some((cached_row, range, len)) = cached
            && *cached_row == tab_row
        {
            let stream_line = fold_rows
                .line(tab_row, self.tab_snapshot.line_count())
                .ok_or(CoordinateError::LineOutOfBounds(Line::new(tab_row)))?;
            return Ok((WrapFragmentKind::Text(stream_line), range.clone(), *len));
        }
        let stream_line = fold_rows
            .line(tab_row, self.tab_snapshot.line_count())
            .ok_or(CoordinateError::LineOutOfBounds(Line::new(tab_row)))?;
        let (range, len) = self.cursor_tab_row_content(tab_row, stream_line, line_cursor)?;
        *cached = Some((tab_row, range.clone(), len));
        Ok((WrapFragmentKind::Text(stream_line), range, len))
    }

    fn row_kind(&self, fragment: WrapFragment) -> DisplayMapResult<WrapRowKind> {
        match fragment.kind {
            WrapFragmentKind::Text(source) => Ok(WrapRowKind::Text {
                source,
                projected_line: fragment.tab_row,
                byte_range: fragment.byte_range,
                content_range: fragment.content_range,
                fragment_index: fragment.fragment_index,
                indent: fragment.indent,
            }),
        }
    }

    pub(super) fn project_text_range(
        &self,
        range: MultiBufferRange,
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
    pub(super) fn beginning_of_row(
        &self,
        offset: MultiBufferOffset,
    ) -> DisplayMapResult<MultiBufferOffset> {
        let point = self.offset_to_wrap_point(offset)?;
        self.wrap_point_to_offset(WrapPoint::new(point.row(), DisplayColumn::ZERO))
    }

    /// 光标所在的显示行行尾（本段末尾，不含换行符）对应的字节偏移。
    pub(super) fn end_of_row(
        &self,
        offset: MultiBufferOffset,
    ) -> DisplayMapResult<MultiBufferOffset> {
        let point = self.offset_to_wrap_point(offset)?;
        let fragment = self.wrap_row_to_fragment(point.row())?;
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
                        fragment.tab_row,
                        &segments,
                        fragment.byte_range.end,
                        FoldBias::Left,
                    );
                }
                Ok(MultiBufferOffset::new(line_start + fragment.byte_range.end))
            }
        }
    }

    /// 显示行 → (tab 行, 片段信息)。
    fn wrap_row_to_fragment(&self, row: WrapRow) -> DisplayMapResult<WrapFragment> {
        let (start, _, transform) =
            self.transforms
                .find::<OutputToInput, _>((), &OutputRows(row.get()), Bias::Right);
        let transform = transform.ok_or(CoordinateError::LineOutOfBounds(Line::new(row.get())))?;
        let input_start = start.1.row();
        let output_start = start.0.0;
        match transform.kind {
            TransformKind::Isomorphic => {
                let tab_row = input_start + (row.get() - output_start);
                let kind = self.projected_kind(tab_row)?;
                let (content_range, content_len) = self.tab_row_content(tab_row)?;
                Ok(WrapFragment {
                    tab_row,
                    kind,
                    byte_range: 0..content_len,
                    content_range,
                    indent: 0,
                    fragment_index: 0,
                })
            }
            TransformKind::Wrap => {
                let kind = self.projected_kind(input_start)?;
                let (content_range, content_len) = self.tab_row_content(input_start)?;
                let fragment_index = row.get() - output_start;
                Ok(WrapFragment {
                    tab_row: input_start,
                    kind,
                    byte_range: fragment_byte_range(
                        &transform.wrap_points,
                        fragment_index,
                        content_len,
                    ),
                    content_range,
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
    fn logical_point_to_wrap_point(
        &self,
        line: Line,
        column: LogicalColumn,
    ) -> DisplayMapResult<WrapPoint> {
        let fold = self.tab_snapshot.fold_snapshot();
        // 隐藏点吸附折叠起点列（光标在折叠内的默认落点）。
        let point =
            fold.logical_to_projected_point(LogicalPoint::new(line, column), FoldBias::Left)?;
        self.projected_point_to_wrap_point(point)
    }

    /// 投影点列 → 行内容字节偏移。
    ///
    /// 折叠合并行与普通行的行文本都由 `line_text` 给出，列是行内字符列；
    /// 换算必须留在「行内容（已剥行终止符）」这一坐标空间内。
    /// 不能经缓冲区 `Position` 往返：
    /// 位置落在 CRLF 的 `\n` 等终止符上时，缓冲区列会越过内容末端，往返会得到行内容之外的字节。
    fn projected_column_to_byte(
        &self,
        line: Line,
        column: LogicalColumn,
    ) -> DisplayMapResult<usize> {
        let text = self
            .tab_snapshot
            .line_text(line)
            .ok_or(CoordinateError::LineOutOfBounds(line))?;
        Ok(byte_after_chars(line_content(text.as_ref()), column.get()))
    }

    fn projected_point_to_wrap_point(&self, point: ProjectedPoint) -> DisplayMapResult<WrapPoint> {
        let tab_row = point.line().get();
        let line = Line::new(tab_row);
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
        debug_assert!(
            content.is_char_boundary(fragment_start) && content.is_char_boundary(target_projected),
            "换行坐标必须落在行内容的字符边界上：fragment_start={fragment_start} target={target_projected} content_len={}",
            content.len()
        );
        // 片段内的显示列从缩进后的列开始累加，tab 对齐基于显示行内列。
        let column = content[fragment_start..target_projected]
            .graphemes(true)
            .fold(indent, |column, grapheme| {
                advance_display_column(column, grapheme, self.tab_snapshot().tab_width().get())
            });
        Ok(WrapPoint::new(
            WrapRow::new(output_start + fragment_index),
            DisplayColumn::new(column),
        ))
    }

    fn transform_for_tab_row(
        &self,
        tab_row: usize,
    ) -> DisplayMapResult<(usize, usize, &Transform)> {
        let (start, _, transform) =
            self.transforms
                .find::<InputToOutput, _>((), &TabPoint::new(tab_row, 0), Bias::Right);
        let transform = transform.ok_or(CoordinateError::LineOutOfBounds(Line::new(tab_row)))?;
        Ok((start.0.row(), start.1.0, transform))
    }

    /// 选区起终点（投影点）→ (显示行, 显示行内显示列)。
    ///
    /// 与 `projected_point_to_wrap_point` 共用同一套显示列语义（Tab 对齐、CJK 宽字符、折叠合并行）；
    /// 渲染端按 `window_start_column` 把显示列换算回行内字节，两者必须同处一个坐标空间。
    fn projected_point_to_range_point(
        &self,
        point: ProjectedPoint,
    ) -> DisplayMapResult<(WrapRow, usize)> {
        let point = self.projected_point_to_wrap_point(point)?;
        Ok((point.row(), point.column().get()))
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

/// 把 Tab 层点编辑扩展为需要重新测量的完整逻辑行。
///
/// 扩展只发生在 Wrap 的消费边界：Tab 层仍保留精确的列端点；本层按行构建
/// soft-wrap 变换，故起止行以及它们的片段都必须失效。相邻编辑在这里合并，
/// 不能再由 TabMap 保存逐行失效集合之类的影子状态。
fn tab_edit_rows(tab_edits: &[TabEdit]) -> Vec<(Range<usize>, Range<usize>)> {
    let mut rows: Vec<_> = tab_edits
        .iter()
        .map(|edit| {
            (
                edit.old.start.row()..edit.old.end.row().saturating_add(1),
                edit.new.start.row()..edit.new.end.row().saturating_add(1),
            )
        })
        .collect();
    rows.sort_by_key(|(old, _)| old.start);

    let mut merged: Vec<(Range<usize>, Range<usize>)> = Vec::with_capacity(rows.len());
    for (old, new) in rows {
        if let Some((previous_old, previous_new)) = merged.last_mut()
            && old.start <= previous_old.end
        {
            previous_old.end = previous_old.end.max(old.end);
            previous_new.end = previous_new.end.max(new.end);
        } else {
            merged.push((old, new));
        }
    }
    merged
}

/// 文本中第 `chars` 个字符的字节偏移（超出末尾返回文本长度）。
/// 一次换行重排影响的显示行区间（换行输出行空间）。
///
/// 无重排时列表为空，因此「空」精确表示显示行布局未变；
/// 有重排时才记录旧/新区间，供 BlockMap 判断块位置是否需要重建。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WrapEdit {
    pub(super) old: Range<usize>,
    pub(super) new: Range<usize>,
}

impl WrapEdit {
    fn old_len(&self) -> usize {
        self.old.end - self.old.start
    }

    fn new_len(&self) -> usize {
        self.new.end - self.new.start
    }
}

/// 显示行坐标上的组合 Patch：把「旧 → 中间」「中间 → 新」两段编辑组合成「旧 → 新」。
///
/// 与 zcv-text 的文本 Patch 使用同一组合算法，只是坐标空间换成换行输出行号。
/// WrapMap 用它记录后台重排期间的急切插值编辑，真实重排落地时先反转再组合，
/// 使 edits_since_sync 始终是从上次对外快照到当前真实快照的净编辑。
#[derive(Debug, Default)]
struct WrapPatch {
    edits: Vec<WrapEdit>,
}

impl WrapPatch {
    fn iter(&self) -> std::slice::Iter<'_, WrapEdit> {
        self.edits.iter()
    }

    fn clear(&mut self) {
        self.edits.clear();
    }

    fn into_inner(self) -> Vec<WrapEdit> {
        self.edits
    }

    fn invert(&mut self) -> &mut Self {
        for edit in &mut self.edits {
            std::mem::swap(&mut edit.old, &mut edit.new);
        }
        self
    }

    fn compose(&self, next: impl IntoIterator<Item = WrapEdit>) -> Self {
        let mut old = self.edits.iter().cloned().peekable();
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
                push_wrap_edit(
                    &mut composed,
                    WrapEdit {
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
                push_wrap_edit(
                    &mut composed,
                    WrapEdit {
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
                push_wrap_edit(
                    &mut composed,
                    WrapEdit {
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
                push_wrap_edit(
                    &mut composed,
                    WrapEdit {
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
                push_wrap_edit(
                    &mut composed,
                    WrapEdit {
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
                push_wrap_edit(
                    &mut composed,
                    WrapEdit {
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

        Self { edits: composed }
    }
}

fn push_wrap_edit(edits: &mut Vec<WrapEdit>, edit: WrapEdit) {
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

/// 给定输入行之前累计的输出行数（换行树的 InputToOutput 维度）。
///
/// 非零输出行的 item 只有 Wrap（输入点恰好跨一行），因此落在 item 内部的行只可能
/// 属于 Isomorphic 段，其增量与输入行偏移一致；用 `Bias::Right` 让区间终点的边界行落到后继 item。
fn output_rows_before(tree: &SumTree<Transform>, input_row: usize) -> usize {
    let mut cursor = tree.cursor::<InputToOutput>(());
    cursor.seek(&TabPoint::new(input_row, 0), Bias::Right);
    let start = cursor.start();
    start.1.0 + (input_row - start.0.row())
}

/// 按 Tab 点顺序映射 Wrap 点，并复用换行树位置与当前行文本。
pub(crate) struct WrapPointCursor<'a> {
    snapshot: &'a WrapSnapshot,
    cursor: sum_tree::Cursor<'a, 'static, Transform, InputToOutput>,
    tab_row: Option<usize>,
    line_text: Option<Cow<'a, str>>,
}

impl WrapPointCursor<'_> {
    pub fn reset(&mut self) {
        self.cursor.reset();
        self.tab_row = None;
        self.line_text = None;
    }

    pub fn map(&mut self, point: TabPointMapping) -> WrapPoint {
        let tab_point = point.point();
        if self.cursor.did_seek() && tab_point >= self.cursor.start().0 {
            self.cursor.seek_forward(&tab_point, Bias::Right);
        } else {
            self.cursor.seek(&tab_point, Bias::Right);
        }

        let Some(transform) = self.cursor.item() else {
            return WrapPoint::new(
                WrapRow::new(self.snapshot.line_count()),
                DisplayColumn::new(tab_point.column()),
            );
        };
        let output_start = self.cursor.start().1.0;
        if matches!(transform.kind, TransformKind::Isomorphic) {
            return WrapPoint::new(
                WrapRow::new(output_start + tab_point.row().saturating_sub(transform.input.row())),
                DisplayColumn::new(tab_point.column()),
            );
        }

        let row = tab_point.row();
        if self.tab_row != Some(row) {
            self.tab_row = Some(row);
            self.line_text = self.snapshot.tab_snapshot.line_text(Line::new(row));
        }
        let Some(text) = self.line_text.as_deref() else {
            return WrapPoint::new(
                WrapRow::new(output_start),
                DisplayColumn::new(tab_point.column()),
            );
        };
        let content = line_content(text);
        let target_byte = point.fold_byte_column().min(content.len());
        let fragment_index = fragment_index_for_byte(&transform.wrap_points, target_byte);
        let fragment_start = fragment_index
            .checked_sub(1)
            .map_or(0, |index| transform.wrap_points[index].byte_ix);
        let indent = fragment_index
            .checked_sub(1)
            .map_or(0, |index| transform.wrap_points[index].indent as usize);
        let column = content[fragment_start..target_byte].graphemes(true).fold(
            indent,
            |column, grapheme| {
                advance_display_column(
                    column,
                    grapheme,
                    self.snapshot.tab_snapshot.tab_width().get(),
                )
            },
        );
        WrapPoint::new(
            WrapRow::new(output_start + fragment_index),
            DisplayColumn::new(column),
        )
    }
}

pub(super) struct WrapMap {
    snapshot: WrapSnapshot,
    wrap_width: Option<Pixels>,
    font_with_size: Option<(Font, Pixels)>,
    /// 由 `set_wrap_width` 缓存；重排时使用同一个 text system 做整行 shaping。
    text_system: Option<Arc<TextSystem>>,
    /// 尚未落地到真实重排的编辑批次（Tab 快照 + Tab 编辑）。
    pending_edits: VecDeque<(TabSnapshot, Vec<TabEdit>)>,
    /// 后台重排期间为保持渲染最新而急切插入的换行编辑；真实重排落地时先反转再组合。
    interpolated_edits: WrapPatch,
    /// 自上次被消费以来发布给下游的换行编辑。
    edits_since_sync: WrapPatch,
    /// 正在进行的后台重排任务。
    background_task: Option<Task<()>>,
    /// 重排配置／任务的代次；旧任务结果只能安装到启动它的代次。
    rewrap_generation: u64,
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
    /// 创建换行层状态。WrapMap 自己拥有配置、变换树、待处理批次与后台任务句柄。
    pub(super) fn new(tab_snapshot: TabSnapshot) -> Self {
        let transforms = isomorphic_tree(&tab_snapshot);
        WrapMap {
            snapshot: WrapSnapshot {
                tab_snapshot,
                transforms,
                wrapped: false,
                interpolated: false,
                version: 0,
            },
            wrap_width: None,
            font_with_size: None,
            text_system: None,
            pending_edits: VecDeque::new(),
            interpolated_edits: WrapPatch::default(),
            edits_since_sync: WrapPatch::default(),
            background_task: None,
            rewrap_generation: 0,
        }
    }

    fn worker(&self) -> WrapWorker {
        let text_system = self
            .text_system
            .as_ref()
            .expect("换行配置必须携带文本系统")
            .clone();
        WrapWorker {
            snapshot: self.snapshot.clone(),
            wrap_width: self.wrap_width.expect("只有开启换行才创建重排任务"),
            font_with_size: self.font_with_size.clone().expect("换行配置必须携带字体"),
            window_text_system: WindowTextSystem::new(text_system.clone()),
            text_system,
            rows_since_yield: 0,
        }
    }

    pub(super) fn snapshot(&self) -> &WrapSnapshot {
        &self.snapshot
    }

    /// 唯一推进入口：入队后先尝试同步完成，超时则后台重排并急切插值。
    ///
    /// 返回当前快照与自上次消费以来的换行编辑；后台未完成时快照是急切插值态。
    pub(super) fn sync(
        &mut self,
        tab_snapshot: TabSnapshot,
        tab_edits: &[TabEdit],
        cx: &mut Context<Self>,
    ) -> (WrapSnapshot, Vec<WrapEdit>) {
        if self.wrap_width.is_none() {
            let old_version = self.snapshot.tab_snapshot.version();
            self.snapshot.tab_snapshot = tab_snapshot;
            if self.snapshot.tab_snapshot.version() != old_version {
                let edits = self.set_isomorphic_all();
                self.edits_since_sync = self.edits_since_sync.compose(edits);
                self.snapshot.version += 1;
            }
        } else {
            // 一个 Tab 版本只保留一份批次；空帧仍采用最新元数据快照。
            if let Some((last_snapshot, _)) = self.pending_edits.back_mut()
                && last_snapshot.version() == tab_snapshot.version()
            {
                debug_assert!(tab_edits.is_empty());
                *last_snapshot = tab_snapshot;
            } else {
                self.pending_edits
                    .push_back((tab_snapshot, tab_edits.to_vec()));
            }
            self.flush_edits(cx);
        }
        debug_assert!(
            self.background_task.is_some()
                || self.wrap_width.is_none()
                || !self.snapshot.interpolated,
            "插值态必须由正在执行的重排任务推进到真实快照"
        );
        (
            self.snapshot.clone(),
            mem::take(&mut self.edits_since_sync).into_inner(),
        )
    }

    /// 同一安装入口处理前台完成与后台完成；只发布相对上次消费的净编辑。
    fn install_rewrap(&mut self, mut snapshot: WrapSnapshot, edits: WrapPatch, covered: usize) {
        if covered > 0 {
            let (latest_tab, _) = &self.pending_edits[covered - 1];
            debug_assert_eq!(snapshot.tab_snapshot.version(), latest_tab.version());
            snapshot.tab_snapshot = latest_tab.clone();
        }
        snapshot.version = self.snapshot.version + 1;
        self.snapshot = snapshot;
        let mut interpolated = mem::take(&mut self.interpolated_edits);
        self.edits_since_sync = self
            .edits_since_sync
            .compose(interpolated.invert().iter().cloned())
            .compose(edits.into_inner());
        self.pending_edits.drain(..covered);
        self.background_task = None;
    }

    fn start_rewrap(
        &mut self,
        pending: Vec<(TabSnapshot, Vec<TabEdit>)>,
        covered: usize,
        budget: Duration,
        cx: &mut Context<Self>,
    ) {
        self.rewrap_generation += 1;
        let generation = self.rewrap_generation;
        let mut worker = self.worker();
        let task = cx.background_spawn(async move {
            let mut edits = WrapPatch::default();
            let mut pending = pending.into_iter().peekable();
            while let Some((tab_snapshot, tab_edits)) = pending.next() {
                edits = edits.compose(worker.apply_edits(tab_snapshot, &tab_edits).await);
                if pending.peek().is_some() {
                    yield_now().await;
                }
            }
            (worker.snapshot, edits)
        });
        match cx.foreground_executor().block_with_timeout(budget, task) {
            Ok((snapshot, edits)) => self.install_rewrap(snapshot, edits, covered),
            Err(task) => {
                self.snapshot.interpolated = true;
                self.background_task = Some(cx.spawn(async move |this, cx| {
                    let (snapshot, edits) = task.await;
                    this.update(cx, |map, cx| {
                        if generation != map.rewrap_generation {
                            return;
                        }
                        map.install_rewrap(snapshot, edits, covered);
                        map.flush_edits(cx);
                        cx.notify();
                    })
                    .ok();
                }));
            }
        }
    }

    fn flush_edits(&mut self, cx: &mut Context<Self>) {
        if !self.snapshot.interpolated {
            let covered = self
                .pending_edits
                .iter()
                .take_while(|(snapshot, _)| {
                    snapshot.version() <= self.snapshot.tab_snapshot.version()
                })
                .count();
            if covered > 0 {
                self.snapshot.tab_snapshot = self.pending_edits[covered - 1].0.clone();
                self.pending_edits.drain(..covered);
            }
        }
        if self.pending_edits.is_empty() {
            return;
        }
        if self.background_task.is_none() {
            let pending = self.pending_edits.iter().cloned().collect();
            self.start_rewrap(
                pending,
                self.pending_edits.len(),
                INCREMENTAL_REWRAP_BUDGET,
                cx,
            );
        }
        for (tab_snapshot, tab_edits) in &self.pending_edits {
            if tab_snapshot.version() <= self.snapshot.tab_snapshot.version() {
                // 同版本批次可能推进纯元数据；不能在后台落地时恢复旧下层快照。
                if tab_snapshot.version() == self.snapshot.tab_snapshot.version() {
                    self.snapshot.tab_snapshot = tab_snapshot.clone();
                }
                continue;
            }
            let edits = self.snapshot.interpolate(tab_snapshot.clone(), tab_edits);
            self.edits_since_sync = self.edits_since_sync.compose(edits.iter().cloned());
            self.interpolated_edits = self.interpolated_edits.compose(edits);
        }
    }

    /// 配置变化取消旧任务，以当前已发布拓扑为基准重排；待消费的净编辑继续累积。
    pub(super) fn set_wrap_width(
        &mut self,
        wrap_width: Option<Pixels>,
        font: Font,
        font_size: Pixels,
        text_system: Arc<TextSystem>,
        cx: &mut Context<Self>,
    ) -> bool {
        let font_changed = self.font_with_size.as_ref() != Some(&(font.clone(), font_size));
        let text_system_changed = self
            .text_system
            .as_ref()
            .is_none_or(|cached| !Arc::ptr_eq(cached, &text_system));
        let changed = wrap_width != self.wrap_width
            || (wrap_width.is_some() && (font_changed || text_system_changed));
        self.font_with_size = Some((font, font_size));
        self.text_system = Some(text_system);
        if !changed {
            return false;
        }
        self.wrap_width = wrap_width;
        self.rewrap_generation += 1;
        self.background_task = None;
        self.pending_edits.clear();
        self.interpolated_edits.clear();
        if wrap_width.is_some() {
            let tab_snapshot = self.snapshot.tab_snapshot.clone();
            let range = TabPoint::zero()..tab_snapshot.max_point();
            self.start_rewrap(
                vec![(
                    tab_snapshot,
                    vec![TabEdit {
                        old: range.clone(),
                        new: range,
                    }],
                )],
                0,
                FULL_REWRAP_BUDGET,
                cx,
            );
        } else {
            let edits = self.set_isomorphic_all();
            self.edits_since_sync = self.edits_since_sync.compose(edits);
            self.snapshot.interpolated = false;
            self.snapshot.version += 1;
        }
        true
    }

    fn set_isomorphic_all(&mut self) -> Vec<WrapEdit> {
        let old_rows = self.snapshot.transforms.summary().output_rows;
        let new_rows = self.snapshot.tab_snapshot.line_count();
        let unchanged = !self.snapshot.wrapped && old_rows == new_rows;
        self.snapshot.transforms = isomorphic_tree(&self.snapshot.tab_snapshot);
        self.snapshot.wrapped = false;
        self.snapshot.check_invariants();
        if unchanged {
            // 文本内容虽然更新，但逐行到显示行的拓扑保持同构；上层只需替换下层快照，
            // 不应把它伪装成显示几何编辑并强制重建 diff 装饰。
            return Vec::new();
        }
        vec![WrapEdit {
            old: 0..old_rows,
            new: 0..new_rows,
        }]
    }
}

/// 重排任务的工作集；不持有实体、队列或通知句柄，测量缓存最多覆盖一个让出批次。
struct WrapWorker {
    snapshot: WrapSnapshot,
    wrap_width: Pixels,
    font_with_size: (Font, Pixels),
    text_system: Arc<TextSystem>,
    window_text_system: WindowTextSystem,
    rows_since_yield: usize,
}

impl WrapWorker {
    async fn apply_edits(
        &mut self,
        tab_snapshot: TabSnapshot,
        tab_edits: &[TabEdit],
    ) -> Vec<WrapEdit> {
        self.snapshot.tab_snapshot = tab_snapshot;
        let edits = self.update_structural(tab_edits).await;
        self.snapshot.interpolated = false;
        self.snapshot.check_invariants();
        self.snapshot.version += 1;
        edits
    }

    /// 结构编辑的局部重排：按 TabEdit 的旧/新输入行区间替换换行变换。
    ///
    /// 未命中的前缀/后缀子树直接复用（Arc 共享）；被替换区间内的行重新测量换行。
    async fn update_structural(&mut self, tab_edits: &[TabEdit]) -> Vec<WrapEdit> {
        let edits = tab_edit_rows(tab_edits);
        if edits.is_empty() {
            // Tab 行拓扑未变（例如只有元数据/语法推进版本）：
            // 保留现有变换树，快照由 apply_edits 更新；不能重建为空树。
            debug_assert_eq!(
                self.snapshot.transforms.summary().input.row(),
                self.snapshot.tab_snapshot.line_count(),
                "Tab 行数变化却没有结构编辑；下层投影链行覆盖不一致"
            );
            return Vec::new();
        }

        // 以输入行为维度的单向前进 splice：
        // 未命中的前缀/后缀子树直接复用（Arc 共享），编辑行重新测量；落
        // 在两编辑之间的旧同构段尾部以同构占位，游标绝不回退。
        let tab_snapshot = self.snapshot.tab_snapshot.clone();
        let mut fold_rows = tab_snapshot.fold_snapshot().rows(edits[0].1.start);
        let mut line_cursor = None;
        let measure = self.snapshot.transforms.clone();
        let old_transforms = std::mem::replace(&mut self.snapshot.transforms, SumTree::new(()));
        let mut cursor = old_transforms.cursor::<TabPoint>(());
        let mut new_tree = SumTree::new(());
        let mut buffered = Vec::new();
        let mut measured = Vec::with_capacity(edits.len());

        let mut edits_iter = edits.iter().peekable();
        if let Some((old_rows, _)) = edits_iter.peek() {
            new_tree.append(
                cursor.slice(&TabPoint::new(old_rows.start, 0), Bias::Right),
                (),
            );
        }
        while let Some((old_rows, new_rows)) = edits_iter.next() {
            // 用新快照补齐「已发出新行 → 编辑新起点」的保留行；
            // 这段只能是旧同构变换内尚未覆盖的行，按同构占位。
            let gap = new_rows
                .start
                .saturating_sub(new_tree.summary().input.row());
            if gap > 0 {
                push_isomorphic(&mut buffered, gap);
            }
            let mut output_rows = 0;
            for tab_row in new_rows.clone() {
                let prepared = Self::prepared_wrap_text(
                    &tab_snapshot,
                    tab_row,
                    &mut fold_rows,
                    &mut line_cursor,
                )
                .expect("已投影的文本行必须能建立塑形输入");
                output_rows += self.push_wrap_transform(&mut buffered, prepared);
                self.rows_since_yield += 1;
                if self.rows_since_yield == WRAP_YIELD_ROW_INTERVAL {
                    // 任务没有窗口帧结束回调；分批释放 shaping 布局，并提供取消检查点。
                    self.window_text_system = WindowTextSystem::new(self.text_system.clone());
                    self.rows_since_yield = 0;
                    yield_now().await;
                }
            }
            measured.push(output_rows);
            new_tree.extend(buffered.drain(..), ());

            // 旧游标只向前推进到编辑终点，不越过包含它的旧变换。
            cursor.seek_forward(&TabPoint::new(old_rows.end, 0), Bias::Right);
            let trailing = if let Some((next_old, _)) = edits_iter.peek() {
                if next_old.start > cursor.end().row() {
                    // 当前旧变换整体落在两编辑之间：尾部以同构占位，随后搬运整段旧变换。
                    if cursor.end().row() > old_rows.end {
                        push_isomorphic(&mut buffered, cursor.end().row() - old_rows.end);
                        new_tree.extend(buffered.drain(..), ());
                    }
                    cursor.next();
                    Some(cursor.slice(&TabPoint::new(next_old.start, 0), Bias::Right))
                } else {
                    // 下一编辑仍在当前旧变换内：其间的同构行由下一轮 gap 补齐。
                    None
                }
            } else {
                if cursor.end().row() > old_rows.end {
                    push_isomorphic(&mut buffered, cursor.end().row() - old_rows.end);
                    new_tree.extend(buffered.drain(..), ());
                }
                cursor.next();
                Some(cursor.suffix())
            };
            if let Some(trailing) = trailing {
                new_tree.append(trailing, ());
            }
            if edits_iter.peek().is_some() {
                yield_now().await;
            }
        }
        debug_assert_eq!(
            new_tree.summary().input.row(),
            self.snapshot.tab_snapshot.line_count(),
            "Wrap 结构重排覆盖不匹配；row_edits={edits:?}"
        );
        self.snapshot.transforms = new_tree;
        self.snapshot.wrapped = true;
        self.snapshot.check_invariants();
        wrap_edits(
            &measure,
            &edits
                .iter()
                .map(|(old_rows, _)| old_rows.clone())
                .collect::<Vec<_>>(),
            &measured,
        )
    }

    /// 计算单个 tab 行的换行变换并压入（相邻 Isomorphic 自动合并）。
    ///
    /// 返回该行贡献的输出显示行数：
    /// 即使它与前一个同构变换合并，调用方仍能按行累计测量值，不依赖压入后的缓冲区切分。
    fn push_wrap_transform(
        &self,
        transforms: &mut Vec<Transform>,
        prepared: PreparedWrapText,
    ) -> usize {
        let boundaries = self.wrap_points(prepared, self.wrap_width);
        if boundaries.is_empty() {
            // 无需软换行：一个输入行对应一个输出行。
            push_isomorphic(transforms, 1);
            1
        } else {
            let output_rows = boundaries.len() + 1;
            transforms.push(Transform {
                kind: TransformKind::Wrap,
                input: TabPoint::new(1, 0),
                output_rows,
                longest_row: 0,
                longest_row_chars: 0,
                wrap_points: boundaries.into(),
            });
            output_rows
        }
    }

    /// 为单个投影行建立文字塑形输入。
    ///
    /// 软换行必须把当前行交给文字系统塑形；
    /// 这里直接消费 Fold 连续 chunk，只保留塑形所需的一份临时文本，不先生成另一份投影整行。
    fn prepared_wrap_text<'a>(
        tab: &'a TabSnapshot,
        tab_row: usize,
        fold_rows: &mut FoldRows<'a>,
        line_cursor: &mut Option<MultiBufferLineCursor<'a>>,
    ) -> DisplayMapResult<PreparedWrapText> {
        let fold = tab.fold_snapshot();
        let tab_width = tab.tab_width().get();
        if let Some(segments) = fold.fold_row_segments(ProjectedLineIndex::new(tab_row)) {
            let content_len = segments
                .last()
                .expect("折叠合并行必须至少包含一个段")
                .merged_range()
                .end;
            return Ok(PreparedWrapText::from_chunks(
                FoldChunks::new(
                    &segments,
                    fold.buffer_snapshot(),
                    HighlightStyles::default(),
                    0..content_len,
                ),
                tab_width,
            ));
        }
        let line = Line::new(tab_row);
        let stream_line = fold_rows
            .line(tab_row, tab.line_count())
            .ok_or(CoordinateError::LineOutOfBounds(line))?;
        let buffer = fold.buffer_snapshot();
        if line_cursor.is_none() {
            *line_cursor = MultiBufferLineCursor::new(buffer, stream_line);
        }
        let cursor = line_cursor
            .as_mut()
            .ok_or(CoordinateError::LineOutOfBounds(line))?;
        if !cursor.seek(stream_line) {
            return Err(CoordinateError::LineOutOfBounds(line).into());
        }
        let (start, content_len) = cursor
            .line_content_range()
            .ok_or(CoordinateError::LineOutOfBounds(line))?;
        let range = MultiBufferOffset::new(start)..MultiBufferOffset::new(start + content_len);
        Ok(PreparedWrapText::from_chunks(
            StyledChunks::new(
                ChunkText::Virtual {
                    snapshot: buffer,
                    range: range.clone(),
                },
                range.start.get(),
                0,
                HighlightStyles::default(),
                0..content_len,
            ),
            tab_width,
        ))
    }

    /// 使用最终字形位置与元素实测宽度计算换行点，避免字符宽度估算与渲染 shaping 使用两套标准。
    ///
    /// 文本原子宽度来自整行 shaping；带 measured_width 的占位符元素作为单个原子宽度参与判定，
    /// 元素内部不产生换行点。
    fn wrap_points(&self, prepared: PreparedWrapText, wrap_width: Pixels) -> Vec<WrapPointInfo> {
        let window_text_system = &self.window_text_system;
        let (font, font_size) = &self.font_with_size;
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

        // 归并为换行原子：文本字符各占一个原子，元素占一个原子并携带实测宽度。
        let mut atoms: Vec<WrapAtom> = Vec::with_capacity(prepared.chars.len());
        let mut prefix_widths: Vec<Pixels> = Vec::with_capacity(prepared.chars.len() + 1);
        prefix_widths.push(Pixels::ZERO);
        let mut char_index = 0usize;
        while char_index < prepared.chars.len() {
            let character = &prepared.chars[char_index];
            let (ch, raw_start, width, next) = match character.element_width {
                Some(width) => (
                    character.ch,
                    character.raw_start,
                    width,
                    character.element_chars,
                ),
                None => (
                    character.ch,
                    character.raw_start,
                    shaped.x_for_index(character.expanded_end)
                        - shaped.x_for_index(character.expanded_start),
                    1,
                ),
            };
            prefix_widths.push(prefix_widths.last().copied().unwrap_or(Pixels::ZERO) + width);
            atoms.push(WrapAtom { ch, raw_start });
            char_index += next;
        }

        let mut points = Vec::new();
        let mut first_non_whitespace: Option<usize> = None;
        let mut indent = None;
        let mut indent_width = Pixels::ZERO;
        let mut last_candidate: Option<usize> = None;
        let mut last_wrap = 0usize;
        let mut line_start_atom = 0usize;
        let mut previous = '\0';

        for (atom_index, atom) in atoms.iter().enumerate() {
            if is_word_char(atom.ch) {
                if previous == ' ' && atom.ch != ' ' && first_non_whitespace.is_some() {
                    last_candidate = Some(atom_index);
                }
            } else if atom.ch != ' ' && first_non_whitespace.is_some() {
                last_candidate = Some(atom_index);
            }

            if atom.ch != ' ' && first_non_whitespace.is_none() {
                first_non_whitespace = Some(atom.raw_start);
            }

            let line_width = prefix_widths[atom_index + 1] - prefix_widths[line_start_atom]
                + if last_wrap > 0 {
                    indent_width
                } else {
                    Pixels::ZERO
                };
            if line_width > wrap_width && atom.raw_start > last_wrap {
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

                let boundary_atom = last_candidate
                    .filter(|candidate| atoms[*candidate].raw_start > last_wrap)
                    .unwrap_or(atom_index);
                points.push(WrapPointInfo {
                    byte_ix: atoms[boundary_atom].raw_start,
                    indent: indent.unwrap_or(0) as u32,
                });
                last_wrap = atoms[boundary_atom].raw_start;
                line_start_atom = boundary_atom;
                last_candidate = None;
            }
            previous = atom.ch;
        }

        points
    }
}

/// 换行原子：一个文本字符，或一个带实测宽度的行内元素。
struct WrapAtom {
    ch: char,
    raw_start: usize,
}

#[derive(Debug)]
struct PreparedWrapChar {
    ch: char,
    raw_start: usize,
    expanded_start: usize,
    expanded_end: usize,
    /// 元素首字符携带该元素实测的像素宽度；元素其余字符为 None。
    element_width: Option<Pixels>,
    /// 元素覆盖的字符数（仅首字符有效）。
    element_chars: usize,
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
        let mut chars: Vec<PreparedWrapChar> = Vec::new();
        let mut column = 0usize;
        let mut raw_start = 0usize;
        // 一个元素可能被 128 字节上限切成多个 chunk：按稳定 id 归并，宽度只计一次。
        let mut current_element_id: Option<ChunkRendererId> = None;
        let mut current_element: Option<(Pixels, usize)> = None;
        for chunk in chunks {
            let element = chunk
                .renderer
                .as_ref()
                .and_then(|renderer| renderer.measured_width.map(|width| (renderer.id, width)));
            if element.map(|(id, _)| id) != current_element_id {
                if let Some((width, first)) = current_element.take() {
                    let count = chars.len() - first;
                    if count > 0 {
                        chars[first].element_width = Some(width);
                        chars[first].element_chars = count;
                    }
                }
                current_element_id = element.map(|(id, _)| id);
                if let Some((_, width)) = element {
                    current_element = Some((width, chars.len()));
                }
            }
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
                    element_width: None,
                    element_chars: 0,
                });
                raw_start += ch.len_utf8();
            }
        }
        if let Some((width, first)) = current_element {
            let count = chars.len() - first;
            if count > 0 {
                chars[first].element_width = Some(width);
                chars[first].element_chars = count;
            }
        }
        Self { text, chars }
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
        last.input = last.input.advance(TabPoint::new(lines, 0));
        last.output_rows += lines;
        return;
    }
    transforms.push(Transform::isomorphic(TabPoint::new(lines, 0), lines));
}

fn isomorphic_tree(tab_snapshot: &TabSnapshot) -> SumTree<Transform> {
    let tab_rows = tab_snapshot.line_count();
    if tab_rows == 0 {
        SumTree::new(())
    } else {
        let input = tab_snapshot.summary_for_range(TabPoint::zero()..TabPoint::new(tab_rows, 0));
        // 透传模式下最长行是显示宽度的派生事实：
        // 构建 summary 时测量一次，查询端按 O(1) 读取，不在每帧重新扫描全部行（对齐 Zed `WrapSummary::longest_row`）。
        let (longest_row, longest_row_chars) = (0..tab_rows)
            .map(|row| display_width_for_fold_row(tab_snapshot, Line::new(row)).unwrap_or_default())
            .enumerate()
            .max_by_key(|(_, width)| *width)
            .unwrap_or((0, 0));
        let mut transform = Transform::isomorphic(input, tab_rows);
        transform.longest_row = longest_row;
        transform.longest_row_chars = longest_row_chars;
        SumTree::from_item(transform, ())
    }
}

#[cfg(test)]
#[path = "test/wrap_map_tests.rs"]
mod tests;
