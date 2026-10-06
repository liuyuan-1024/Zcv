//! DisplayMap 的软换行（soft wrap）层。
//!
//! WrapMap 在 TabMap 之上，把超过指定像素宽度的逻辑行拆成多个显示行。
//! 换行点由 GPUI LineWrapper 顺序消费 Tab 展开的文本与实测元素片段，按词边界、长词硬断和续行缩进规则计算。
//! 续行的视觉缩进是一段"假空格"，作为显示文本的前缀参与布局、命中测试与坐标换算，因此渲染端无需为续行做任何特殊定位。
//!
//! 与 FoldMap 一样，WrapMap 用 `SumTree<Transform>` 维护"输入 tab 行 → 输出显示行"的拓扑：Isomorphic 段把连续不换行行合并，Wrap 段把单个宽行拆成 `wrap_points.len() + 1` 个显示行。
//! 变换树在装配时保持规范形：相邻同构段必须归并，由 `push_isomorphic` 与 `append_canonical` 统一维护并在 `check_invariants` 中校验。
//! 折叠与换行是正交的两层变换：折叠先塌缩文本，换行再按像素宽度切分。

use zcv_multi_buffer::{MultiBufferOffset, MultiBufferRange};

use std::borrow::Cow;
use std::collections::VecDeque;
use std::mem;
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use futures_lite::future::yield_now;
use gpui::{
    AppContext as _, Context, Font, LineFragment, LineWrapperHandle, Pixels, Task, TextSystem,
};
use sum_tree::{Bias, ContextLessSummary, Dimension, Dimensions, Item, SumTree};
use unicode_segmentation::UnicodeSegmentation;
use zcv_multi_buffer::{ExcerptSnapshot, MultiBufferLineCursor, MultiBufferSnapshot};
use zcv_text::{CoordinateError, Line, LogicalColumn};

use super::chunk::{Chunk, ChunkText, FoldChunks, HighlightStyles, StyledChunks};
use super::display_width::DisplayColumn;
use super::error::DisplayMapResult;
use super::fold_map::{
    ChunkRendererId, FoldBias, FoldOffset, FoldRowSegment, FoldRowSegmentKind, FoldRows,
    LogicalPoint, LogicalRange, ProjectedLineIndex, ProjectedPoint, StreamProjectedKind,
};
use super::tab_map::{
    TabEdit, TabPoint, TabPointMapping, TabSnapshot, advance_display_column,
    byte_for_display_column, line_content,
};
use super::{WrapPoint, WrapRange, WrapRow};

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
    /// 是否只是按编辑急切插值、尚未经过真实换行测量（由后台任务补齐）。
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
            let mut previous_isomorphic = false;
            for transform in self.transforms.iter() {
                match transform.kind {
                    TransformKind::Isomorphic => {
                        assert!(!previous_isomorphic, "Wrap 变换树不得包含相邻同构段");
                        assert!(transform.input.row() > 0);
                        previous_isomorphic = true;
                    }
                    TransformKind::Wrap => {
                        assert_eq!(transform.input, TabPoint::new(1, 0));
                        assert!(!transform.wrap_points.is_empty());
                        previous_isomorphic = false;
                    }
                }
            }
        }
    }

    pub(super) fn version(&self) -> u64 {
        self.version
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

    /// 按编辑急切插值：结构编辑区间用 isomorphic 段占位，不重新测量换行。
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
        let mut old_ranges = Vec::with_capacity(structural.len());
        let mut measured = Vec::with_capacity(structural.len());

        let mut edits = structural.iter().peekable();
        if let Some((old_rows, _)) = edits.peek() {
            append_canonical(
                &mut new_tree,
                cursor.slice(&TabPoint::new(old_rows.start, 0), Bias::Right),
            );
        }
        while let Some((old_rows, new_rows)) = edits.next() {
            // 用新快照补齐「已发出新行 → 编辑新起点」的保留行。
            let gap = new_rows
                .start
                .saturating_sub(new_tree.summary().input.row());
            if gap > 0 {
                push_isomorphic(&mut new_tree, gap);
            }
            // 急切插值：新行按同构占位，等待后台真实重排。
            push_isomorphic(&mut new_tree, new_rows.len());
            old_ranges.push(old_rows.clone());
            measured.push(new_rows.len());

            // 旧游标只向前推进到编辑终点，不越过包含它的旧变换。
            cursor.seek_forward(&TabPoint::new(old_rows.end, 0), Bias::Right);
            // 编辑终点恰好落在未编辑 Wrap 行的行首时，保留这条完整变换。
            // 其他情况沿用同构段尾部占位，避免从旧变换内部复制已编辑范围。
            let trailing = if cursor.start().row() == old_rows.end
                && cursor
                    .item()
                    .is_some_and(|transform| transform.kind == TransformKind::Wrap)
            {
                if let Some((next_old, _)) = edits.peek() {
                    Some(cursor.slice(&TabPoint::new(next_old.start, 0), Bias::Right))
                } else {
                    Some(cursor.suffix())
                }
            } else if let Some((next_old, _)) = edits.peek() {
                if next_old.start > cursor.end().row() {
                    if cursor.end().row() > old_rows.end {
                        push_isomorphic(&mut new_tree, cursor.end().row() - old_rows.end);
                    }
                    cursor.next();
                    Some(cursor.slice(&TabPoint::new(next_old.start, 0), Bias::Right))
                } else {
                    None
                }
            } else {
                if cursor.end().row() > old_rows.end {
                    push_isomorphic(&mut new_tree, cursor.end().row() - old_rows.end);
                }
                cursor.next();
                Some(cursor.suffix())
            };
            if let Some(trailing) = trailing {
                append_canonical(&mut new_tree, trailing);
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
    ) -> DisplayMapResult<Vec<WrapRange>> {
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

        Ok(breakpoints
            .windows(2)
            .filter(|window| window[0] != window[1])
            .map(|window| {
                WrapRange::new(
                    WrapPoint::new(window[0].0, DisplayColumn::new(window[0].1)),
                    WrapPoint::new(window[1].0, DisplayColumn::new(window[1].1)),
                )
            })
            .collect())
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

    fn projected_point_to_wrap_point(&self, point: ProjectedPoint) -> DisplayMapResult<WrapPoint> {
        let tab_point = self.tab_snapshot.point_cursor().map(point);
        self.point_cursor().map(tab_point)
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
        let old_range = old_before..old_after;
        let new_range = start..start + new_len;
        result.push(WrapEdit::new(old_range, new_range));
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

/// 一次换行重排影响的显示行区间（换行输出行空间）。
///
/// 无重排时列表为空，因此「空」精确表示显示行布局未变；
/// 有重排时才记录旧/新区间，供 BlockMap 判断块位置是否需要重建。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct WrapEdit {
    pub(super) old: Range<WrapRow>,
    pub(super) new: Range<WrapRow>,
}

impl WrapEdit {
    pub(super) fn new(old: Range<usize>, new: Range<usize>) -> Self {
        Self {
            old: WrapRow::new(old.start)..WrapRow::new(old.end),
            new: WrapRow::new(new.start)..WrapRow::new(new.end),
        }
    }

    fn old_len(&self) -> usize {
        self.old.end.get() - self.old.start.get()
    }

    fn new_len(&self) -> usize {
        self.new.end.get() - self.new.start.get()
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
                let unchanged = edit.old.start.get() - old_position;
                old_position += unchanged;
                new_position += unchanged;
                push_wrap_edit(
                    &mut composed,
                    WrapEdit::new(
                        old_position..old_position + edit.old_len(),
                        new_position..new_position + edit.new_len(),
                    ),
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
                let unchanged = edit.new.start.get() - new_position;
                old_position += unchanged;
                new_position += unchanged;
                push_wrap_edit(
                    &mut composed,
                    WrapEdit::new(
                        old_position..old_position + edit.old_len(),
                        new_position..new_position + edit.new_len(),
                    ),
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
                let unchanged = old_edit.old.start.get() - old_position;
                old_position += unchanged;
                new_position += unchanged;
                let overlap_offset = next_edit.old.start.get() - old_edit.new.start.get();
                let old_end = (old_position + overlap_offset).min(old_edit.old.end.get());
                let new_end = new_position + overlap_offset;
                push_wrap_edit(
                    &mut composed,
                    WrapEdit::new(old_position..old_end, new_position..new_end),
                );
                old_edit.old.start = WrapRow::new(old_end);
                old_edit.new.start = WrapRow::new(old_edit.new.start.get() + overlap_offset);
                old_position = old_end;
                new_position = new_end;
            } else {
                let unchanged = next_edit.new.start.get() - new_position;
                old_position += unchanged;
                new_position += unchanged;
                let overlap_offset = old_edit.new.start.get() - next_edit.old.start.get();
                let old_end = old_position + overlap_offset;
                let new_end = (new_position + overlap_offset).min(next_edit.new.end.get());
                push_wrap_edit(
                    &mut composed,
                    WrapEdit::new(old_position..old_end, new_position..new_end),
                );
                next_edit.old.start = WrapRow::new(next_edit.old.start.get() + overlap_offset);
                next_edit.new.start = WrapRow::new(new_end);
                old_position = old_end;
                new_position = new_end;
            }

            if old_edit.new.end > next_edit.old.end {
                let old_end = old_position + old_edit.old_len().min(next_edit.old_len());
                let new_end = new_position + next_edit.new_len();
                push_wrap_edit(
                    &mut composed,
                    WrapEdit::new(old_position..old_end, new_position..new_end),
                );
                old_edit.old.start = WrapRow::new(old_end);
                old_edit.new.start = next_edit.old.end;
                old_position = old_end;
                new_position = new_end;
                next.next();
            } else {
                let old_end = old_position + old_edit.old_len();
                let new_end = new_position + old_edit.new_len().min(next_edit.new_len());
                push_wrap_edit(
                    &mut composed,
                    WrapEdit::new(old_position..old_end, new_position..new_end),
                );
                next_edit.old.start = old_edit.new.end;
                next_edit.new.start = WrapRow::new(new_end);
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

    pub fn map(&mut self, point: TabPointMapping) -> DisplayMapResult<WrapPoint> {
        let tab_point = point.point();
        if self.cursor.did_seek() && tab_point >= self.cursor.start().0 {
            self.cursor.seek_forward(&tab_point, Bias::Right);
        } else {
            self.cursor.seek(&tab_point, Bias::Right);
        }

        let transform = self
            .cursor
            .item()
            .ok_or(CoordinateError::LineOutOfBounds(Line::new(tab_point.row())))?;
        let output_start = self.cursor.start().1.0;
        if matches!(transform.kind, TransformKind::Isomorphic) {
            return Ok(WrapPoint::new(
                WrapRow::new(output_start + tab_point.row() - self.cursor.start().0.row()),
                DisplayColumn::new(tab_point.column()),
            ));
        }

        let row = tab_point.row();
        if self.tab_row != Some(row) {
            self.tab_row = Some(row);
            self.line_text = self.snapshot.tab_snapshot.line_text(Line::new(row));
        }
        let text = self
            .line_text
            .as_deref()
            .ok_or(CoordinateError::LineOutOfBounds(Line::new(row)))?;
        let content = line_content(text);
        let target_byte = point.fold_byte_column();
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
        Ok(WrapPoint::new(
            WrapRow::new(output_start + fragment_index),
            DisplayColumn::new(column),
        ))
    }
}

pub(super) struct WrapMap {
    snapshot: WrapSnapshot,
    wrap_width: Option<Pixels>,
    font_with_size: Option<(Font, Pixels)>,
    /// 由换行配置持有；重排任务从同一文本系统借用字体换行器。
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
            line_wrapper: {
                let (font, font_size) = self.font_with_size.clone().expect("换行配置必须携带字体");
                text_system.line_wrapper(font, font_size)
            },
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
                let row_edits: Vec<_> = tab_edits
                    .iter()
                    .filter(|edit| {
                        edit.old.start.row() != edit.old.end.row()
                            || edit.new.start.row() != edit.new.end.row()
                    })
                    .cloned()
                    .collect();
                let edits = tab_edit_rows(&row_edits)
                    .into_iter()
                    .map(|(old, new)| WrapEdit::new(old, new))
                    .collect::<Vec<_>>();
                debug_assert!(
                    !edits.is_empty()
                        || self.snapshot.transforms.summary().output_rows
                            == self.snapshot.tab_snapshot.line_count(),
                    "Tab 行数变化必须携带结构编辑"
                );
                self.snapshot.transforms = isomorphic_tree(&self.snapshot.tab_snapshot);
                self.snapshot.check_invariants();
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
        vec![WrapEdit::new(0..old_rows, 0..new_rows)]
    }
}

/// 重排任务独占从文本系统借用的换行器；完成或取消时归还字体宽度缓存，不保留整行塑形结果。
struct WrapWorker {
    snapshot: WrapSnapshot,
    wrap_width: Pixels,
    line_wrapper: LineWrapperHandle,
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
        let mut measured = Vec::with_capacity(edits.len());

        let mut edits_iter = edits.iter().peekable();
        if let Some((old_rows, _)) = edits_iter.peek() {
            append_canonical(
                &mut new_tree,
                cursor.slice(&TabPoint::new(old_rows.start, 0), Bias::Right),
            );
        }
        while let Some((old_rows, new_rows)) = edits_iter.next() {
            // 用新快照补齐「已发出新行 → 编辑新起点」的保留行。
            let gap = new_rows
                .start
                .saturating_sub(new_tree.summary().input.row());
            if gap > 0 {
                push_isomorphic(&mut new_tree, gap);
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
                output_rows += self.push_wrap_transform(&mut new_tree, prepared);
                self.rows_since_yield += 1;
                if self.rows_since_yield == WRAP_YIELD_ROW_INTERVAL {
                    // 分批让出执行权，使任务替换与实体销毁能够及时取消重排。
                    self.rows_since_yield = 0;
                    yield_now().await;
                }
            }
            measured.push(output_rows);

            // 旧游标只向前推进到编辑终点，不越过包含它的旧变换。
            cursor.seek_forward(&TabPoint::new(old_rows.end, 0), Bias::Right);
            // 编辑终点恰好落在未编辑 Wrap 行的行首时，保留这条完整变换。
            // 其他情况沿用同构段尾部占位，避免从旧变换内部复制已编辑范围。
            let trailing = if cursor.start().row() == old_rows.end
                && cursor
                    .item()
                    .is_some_and(|transform| transform.kind == TransformKind::Wrap)
            {
                if let Some((next_old, _)) = edits_iter.peek() {
                    Some(cursor.slice(&TabPoint::new(next_old.start, 0), Bias::Right))
                } else {
                    Some(cursor.suffix())
                }
            } else if let Some((next_old, _)) = edits_iter.peek() {
                if next_old.start > cursor.end().row() {
                    if cursor.end().row() > old_rows.end {
                        push_isomorphic(&mut new_tree, cursor.end().row() - old_rows.end);
                    }
                    cursor.next();
                    Some(cursor.slice(&TabPoint::new(next_old.start, 0), Bias::Right))
                } else {
                    None
                }
            } else {
                if cursor.end().row() > old_rows.end {
                    push_isomorphic(&mut new_tree, cursor.end().row() - old_rows.end);
                }
                cursor.next();
                Some(cursor.suffix())
            };
            if let Some(trailing) = trailing {
                append_canonical(&mut new_tree, trailing);
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
        &mut self,
        transforms: &mut SumTree<Transform>,
        prepared: PreparedWrapText,
    ) -> usize {
        let boundaries = self.wrap_points(prepared, self.wrap_width);
        if boundaries.is_empty() {
            // 无需软换行：一个输入行对应一个输出行。
            push_isomorphic(transforms, 1);
            1
        } else {
            let output_rows = boundaries.len() + 1;
            transforms.push(
                Transform {
                    kind: TransformKind::Wrap,
                    input: TabPoint::new(1, 0),
                    output_rows,
                    longest_row: 0,
                    longest_row_chars: 0,
                    wrap_points: boundaries.into(),
                },
                (),
            );
            output_rows
        }
    }

    /// 从 Fold 连续 chunk 建立一行的换行片段，保留 Tab 展开与源字节边界的映射。
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

    /// GPUI 负责顺序测量、断词与缩进；本层只把其字节边界转换回 Fold 行的源字节空间。
    fn wrap_points(
        &mut self,
        prepared: PreparedWrapText,
        wrap_width: Pixels,
    ) -> Vec<WrapPointInfo> {
        let mut fragments = Vec::new();
        let mut text_start = 0;
        let mut index = 0;
        while index < prepared.chars.len() {
            let character = &prepared.chars[index];
            if let Some(width) = character.element_width {
                if text_start < character.expanded_start {
                    fragments.push(LineFragment::text(
                        &prepared.text[text_start..character.expanded_start],
                    ));
                }
                let end = prepared.chars[index + character.element_chars - 1].expanded_end;
                fragments.push(LineFragment::element(width, end - character.expanded_start));
                text_start = end;
                index += character.element_chars;
            } else {
                index += 1;
            }
        }
        if text_start < prepared.text.len() {
            fragments.push(LineFragment::text(&prepared.text[text_start..]));
        }
        let mut points: Vec<WrapPointInfo> = Vec::new();
        let mut character_index = 0;
        for boundary in self.line_wrapper.wrap_line(&fragments, wrap_width) {
            while character_index < prepared.chars.len()
                && prepared.chars[character_index].expanded_start < boundary.ix
            {
                character_index += 1;
            }
            let byte_ix = prepared
                .chars
                .get(character_index)
                .map_or(prepared.raw_len, |character| character.raw_start);
            // Tab 的多个展开字节属于同一个源字符；向右吸附后只保留非空源片段边界。
            if byte_ix < prepared.raw_len
                && byte_ix > points.last().map_or(0, |point| point.byte_ix)
            {
                points.push(WrapPointInfo {
                    byte_ix,
                    indent: boundary.next_indent,
                });
            }
        }
        points
    }
}

#[derive(Debug)]
struct PreparedWrapChar {
    raw_start: usize,
    expanded_start: usize,
    expanded_end: usize,
    /// 元素首字符携带该元素实测的像素宽度；元素其余字符为 None。
    element_width: Option<Pixels>,
    /// 元素覆盖的字符数（仅首字符有效）。
    element_chars: usize,
}

/// 把 Tab 按下层列规则展开，保留换行文本到源字节的边界映射与实测元素。
#[derive(Debug)]
struct PreparedWrapText {
    text: String,
    chars: Vec<PreparedWrapChar>,
    raw_len: usize,
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
        Self {
            text,
            chars,
            raw_len: raw_start,
        }
    }
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

/// 追加以 `lines` 行为输入与输出的同构段；与树尾同构段合并，维持规范形。
fn push_isomorphic(transforms: &mut SumTree<Transform>, lines: usize) {
    let mut merged = false;
    transforms.update_last(
        |last| {
            if last.kind == TransformKind::Isomorphic {
                last.input = last.input.advance(TabPoint::new(lines, 0));
                last.output_rows += lines;
                merged = true;
            }
        },
        (),
    );
    if !merged {
        transforms.push(Transform::isomorphic(TabPoint::new(lines, 0), lines), ());
    }
}

/// 把 `incoming` 追加到变换树，并归并交界处的相邻同构段，维持规范形。
///
/// `tree` 与 `incoming` 内部都已是规范形，因此只可能在两棵树的交界处产生
/// 相邻同构段；这里只处理该处，未变的前缀与后缀子树仍按 Arc 共享，不做整树重建。
fn append_canonical(tree: &mut SumTree<Transform>, incoming: SumTree<Transform>) {
    if incoming.is_empty() {
        return;
    }
    let merges = tree
        .last()
        .is_some_and(|last| last.kind == TransformKind::Isomorphic)
        && incoming
            .first()
            .is_some_and(|first| first.kind == TransformKind::Isomorphic);
    if !merges {
        tree.append(incoming, ());
        return;
    }

    let first = incoming.first().cloned().expect("incoming 非空必含首项");
    let mut cursor = incoming.cursor::<TabPoint>(());
    // slice 取走首项并推进游标；首项并入树尾，其余段原样追加。
    let _ = cursor.slice(&first.input, Bias::Right);
    let rest = cursor.suffix();
    drop(cursor);

    tree.update_last(
        |last| {
            let old_rows = last.output_rows;
            last.input = last.input.advance(first.input);
            last.output_rows = old_rows + first.output_rows;
            if first.longest_row_chars > last.longest_row_chars {
                last.longest_row = old_rows + first.longest_row;
                last.longest_row_chars = first.longest_row_chars;
            }
        },
        (),
    );
    tree.append(rest, ());
}

fn isomorphic_tree(tab_snapshot: &TabSnapshot) -> SumTree<Transform> {
    let tab_rows = tab_snapshot.line_count();
    if tab_rows == 0 {
        SumTree::new(())
    } else {
        let input = tab_snapshot.summary_for_range(TabPoint::zero()..TabPoint::new(tab_rows, 0));
        // 透传模式下最长行来自下层文本摘要（按行索引随编辑增量维护）：
        // 查询端按 O(1) 读取，不在每次同步重新扫描全部行。
        let summary = tab_snapshot.fold_snapshot().text_summary();
        let mut transform = Transform::isomorphic(input, tab_rows);
        transform.longest_row = summary.longest_row;
        transform.longest_row_chars = summary.longest_row_chars;
        SumTree::from_item(transform, ())
    }
}

#[cfg(test)]
#[path = "test/wrap_map_tests.rs"]
mod tests;
