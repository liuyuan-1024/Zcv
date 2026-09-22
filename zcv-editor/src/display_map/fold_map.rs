//! 折叠显示层。
//!
//! FoldMap 是唯一写入口，向上层发布 FoldSnapshot 与 FoldEdit。
//!
//! 折叠模型对齐 Zed 的字节偏移文本变换：变换树的输入是组合文本字节，输出是折叠后的字节；
//! 每个折叠变换把被折区间替换为占位符文本，区间前后的文本是相邻的同构变换。
//! 折叠段不再产生投影行，合并行由输出变换自然给出（anchor 文本 + 占位符 + 闭合行尾段）。
//! 折叠编辑按旧/新变换树映射到精确的输出字节区间，不依赖全局行数守恒锚点。

use zcv_multi_buffer::{MultiBufferAnchor, MultiBufferOffset, MultiBufferRange};

use std::any::TypeId;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::{Deref, DerefMut, Range};
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{AnyElement, App, Pixels, div};
use sum_tree::{Bias as TreeBias, ContextLessSummary, Dimension, Dimensions, Item, SumTree};
use zcv_multi_buffer::MBTextSummary;
use zcv_multi_buffer::MultiBufferSnapshot;
use zcv_text::{Affinity, CoordinateError, Line, LineRange, LogicalColumn, Position};

use super::edit::ProjectionEdit;
use super::error::{DisplayMapResult, FoldError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct FoldId(u64);

impl FoldId {
    const INITIAL: Self = Self(1);
}

/// 渲染替换的稳定身份；折叠是当前唯一来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ChunkRendererId {
    Fold(FoldId),
}

/// 一段文本被替换为自定义元素时的渲染描述。
///
/// 对齐 Zed `ChunkRenderer` 的渲染契约：`render` 产出元素，`constrain_width` 决定它是否按占位符文本的塑形宽度约束。
#[derive(Clone)]
pub(crate) struct ChunkRenderer {
    pub(crate) id: ChunkRendererId,
    pub(crate) render: Arc<dyn Send + Sync + Fn(&mut App) -> AnyElement>,
    pub(crate) constrain_width: bool,
    /// 上一帧布局实测的元素宽度；渲染层在 prepaint 后按 id 回写，换行层据此度量。
    ///
    /// None 表示尚未布局过。权威是折叠层的 fold_metadata_by_id，渲染层不保存第二份。
    pub(crate) measured_width: Option<Pixels>,
}

impl std::fmt::Debug for ChunkRenderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChunkRenderer")
            .field("id", &self.id)
            .field("constrain_width", &self.constrain_width)
            .field("measured_width", &self.measured_width)
            .finish()
    }
}

impl PartialEq for ChunkRenderer {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.constrain_width == other.constrain_width
            && Arc::ptr_eq(&self.render, &other.render)
            && self.measured_width == other.measured_width
    }
}

impl Eq for ChunkRenderer {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Fold {
    id: FoldId,
    /// 折叠端点的长期表示：组合锚点，跨文本编辑与投影重建由当前快照解析。
    range: FoldRange,
    /// 折叠占位符描述；折叠创建时给定，跨重解析保留。
    placeholder: FoldPlaceholder,
}

impl Fold {
    /// 用当前快照把一段组合字节范围折叠为锚点表示。
    fn from_text_range(
        snapshot: &MultiBufferSnapshot,
        id: FoldId,
        range: MultiBufferRange,
        placeholder: FoldPlaceholder,
    ) -> Self {
        Self {
            id,
            // range_inside 语义：起点贴插入之后、终点贴插入之前，折叠不吸收边界插入。
            range: FoldRange(
                snapshot.anchor_at(range.start(), Affinity::After)
                    ..snapshot.anchor_at(range.end(), Affinity::Before),
            ),
            placeholder,
        }
    }

    /// 用新快照按锚点重新解析折叠范围；锚点已退出投影或范围退化时返回 None。
    fn resolve(&self, snapshot: &MultiBufferSnapshot) -> Option<ResolvedFold> {
        let range = resolve_fold_range(snapshot, &self.range)?;
        let line_span = fold_line_span(snapshot, range).ok()?;
        Some(ResolvedFold {
            fold: self.clone(),
            text_range: range,
            line_span,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedFold {
    fold: Fold,
    text_range: MultiBufferRange,
    line_span: (Line, Line),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FoldRange(Range<MultiBufferAnchor>);

impl Deref for FoldRange {
    type Target = Range<MultiBufferAnchor>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for FoldRange {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Default for FoldRange {
    fn default() -> Self {
        Self(MultiBufferAnchor::Min..MultiBufferAnchor::Max)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FoldSummary {
    start: MultiBufferAnchor,
    end: MultiBufferAnchor,
    min_start: MultiBufferAnchor,
    max_end: MultiBufferAnchor,
    count: usize,
}

impl Default for FoldSummary {
    fn default() -> Self {
        Self {
            start: MultiBufferAnchor::Min,
            end: MultiBufferAnchor::Max,
            min_start: MultiBufferAnchor::Max,
            max_end: MultiBufferAnchor::Min,
            count: 0,
        }
    }
}

impl sum_tree::Summary for FoldSummary {
    type Context<'a> = &'a MultiBufferSnapshot;

    fn zero<'a>(_snapshot: Self::Context<'a>) -> Self {
        Self::default()
    }

    fn add_summary<'a>(&mut self, summary: &Self, snapshot: Self::Context<'a>) {
        if anchor_cmp(&summary.min_start, &self.min_start, snapshot) == Ordering::Less {
            self.min_start = summary.min_start;
        }
        if anchor_cmp(&summary.max_end, &self.max_end, snapshot) == Ordering::Greater {
            self.max_end = summary.max_end;
        }
        #[cfg(debug_assertions)]
        {
            let start_comparison = anchor_cmp(&self.start, &summary.start, snapshot);
            assert!(
                start_comparison <= Ordering::Equal,
                "折叠锚点表必须按起点升序汇总"
            );
            if start_comparison == Ordering::Equal {
                assert!(
                    anchor_cmp(&self.end, &summary.end, snapshot) >= Ordering::Equal,
                    "同起点的折叠必须按终点降序汇总"
                );
            }
        }
        self.count += summary.count;
        self.start = summary.start;
        self.end = summary.end;
    }
}

impl Item for Fold {
    type Summary = FoldSummary;

    fn summary(&self, _snapshot: &MultiBufferSnapshot) -> Self::Summary {
        FoldSummary {
            start: self.range.start,
            end: self.range.end,
            min_start: self.range.start,
            max_end: self.range.end,
            count: 1,
        }
    }
}

impl<'a> Dimension<'a, FoldSummary> for FoldRange {
    fn zero(_snapshot: &MultiBufferSnapshot) -> Self {
        Self(MultiBufferAnchor::Min..MultiBufferAnchor::Max)
    }

    fn add_summary(&mut self, summary: &'a FoldSummary, _snapshot: &MultiBufferSnapshot) {
        self.0.start = summary.start;
        self.0.end = summary.end;
    }
}

impl<'a> sum_tree::SeekTarget<'a, FoldSummary, FoldRange> for FoldRange {
    fn cmp(&self, cursor_location: &FoldRange, snapshot: &MultiBufferSnapshot) -> Ordering {
        anchor_cmp(&self.start, &cursor_location.start, snapshot)
            .then_with(|| anchor_cmp(&cursor_location.end, &self.end, snapshot))
    }
}

/// 折叠树按 Anchor 顺序的项目索引；只用于批量 splice，不是折叠的第二份状态。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct FoldIndex(usize);

impl<'a> Dimension<'a, FoldSummary> for FoldIndex {
    fn zero(_snapshot: &MultiBufferSnapshot) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &'a FoldSummary, _snapshot: &MultiBufferSnapshot) {
        self.0 += summary.count;
    }
}

fn anchor_cmp(
    left: &MultiBufferAnchor,
    right: &MultiBufferAnchor,
    snapshot: &MultiBufferSnapshot,
) -> Ordering {
    left.cmp(right, snapshot)
}

/// 隐藏点投影的 bias 约定：Left 吸附折叠起点列，Right 吸附折叠终点列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FoldBias {
    Left,
    Right,
}

/// 默认折叠占位符文本。
pub(crate) const FOLD_PLACEHOLDER: &str = "\u{22ef}";

/// 折叠占位符描述，对齐 Zed `FoldPlaceholder`。
///
/// `render` 由调用方提供，用来把折叠区间渲染成任意元素；
/// `collapsed_text` 为 None 时使用默认省略号；
/// `constrain_width` 决定元素是否按占位符文本宽度约束；
/// `merge_adjacent` 控制相邻折叠是否合并为一段；
/// `type_tag` 用于按类别移除折叠。
/// 折叠占位符的渲染回调：按稳定身份与锚点范围产出元素。
pub(crate) type FoldRenderer =
    Arc<dyn Send + Sync + Fn(FoldId, Range<MultiBufferAnchor>, &mut App) -> AnyElement>;

#[derive(Clone)]
pub(crate) struct FoldPlaceholder {
    /// 把折叠渲染成元素；默认渲染空元素，与 Zed 默认实现一致。
    pub(crate) render: FoldRenderer,
    pub(crate) collapsed_text: Option<Arc<str>>,
    pub(crate) constrain_width: bool,
    pub(crate) merge_adjacent: bool,
    pub(crate) type_tag: Option<TypeId>,
}

impl Default for FoldPlaceholder {
    fn default() -> Self {
        Self {
            render: Arc::new(|_, _, _| gpui::Empty.into_any_element()),
            collapsed_text: None,
            constrain_width: true,
            merge_adjacent: true,
            type_tag: None,
        }
    }
}

impl std::fmt::Debug for FoldPlaceholder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FoldPlaceholder")
            .field("constrain_width", &self.constrain_width)
            .field("collapsed_text", &self.collapsed_text)
            .field("merge_adjacent", &self.merge_adjacent)
            .finish()
    }
}

impl PartialEq for FoldPlaceholder {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.render, &other.render)
            && self.collapsed_text == other.collapsed_text
            && self.constrain_width == other.constrain_width
            && self.merge_adjacent == other.merge_adjacent
            && self.type_tag == other.type_tag
    }
}

impl Eq for FoldPlaceholder {}

impl FoldPlaceholder {
    pub(crate) fn text(&self) -> &str {
        self.collapsed_text.as_deref().unwrap_or(FOLD_PLACEHOLDER)
    }

    /// 编辑器默认的可见占位符：把省略号渲染成带占位色的文本元素。
    ///
    /// `Default::default()` 与 Zed 一致渲染空元素；
    /// 需要可见折叠提示的编辑器折叠入口使用本构造。
    pub(crate) fn ellipsis() -> Self {
        Self {
            render: Arc::new(|_, _, cx: &mut App| {
                div()
                    .text_color(zcv_theme::color::current(cx).text_placeholder)
                    .child(FOLD_PLACEHOLDER)
                    .into_any_element()
            }),
            ..Self::default()
        }
    }
}

/// 折叠变换树中的占位符：被折区间在输出空间中的替代文本与渲染描述。
#[derive(Clone, Debug, PartialEq, Eq)]
struct TransformPlaceholder {
    text: Arc<str>,
    renderer: ChunkRenderer,
}

/// 一段文本变换：输入是组合文本字节，输出是投影字节。
///
/// placeholder 为 None 表示同构（输入输出等长）；Some 表示折叠变换（输出为占位符文本）。
#[derive(Clone, Debug)]
struct Transform {
    summary: TransformSummary,
    placeholder: Option<TransformPlaceholder>,
}

impl Transform {
    fn is_fold(&self) -> bool {
        self.placeholder.is_some()
    }
}

/// 变换的输入/输出文本多维摘要。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TransformSummary {
    input: MBTextSummary,
    output: MBTextSummary,
}

impl ContextLessSummary for TransformSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.input += summary.input;
        self.output += summary.output;
    }
}

impl Item for Transform {
    type Summary = TransformSummary;

    fn summary(&self, (): ()) -> Self::Summary {
        self.summary.clone()
    }
}

/// 变换树输入行数维度（换行数）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct InputLines(usize);

impl<'a> Dimension<'a, TransformSummary> for InputLines {
    fn zero((): ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &'a TransformSummary, (): ()) {
        self.0 += summary.input.lines;
    }
}

/// 变换树输出行数维度（换行数）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct OutputRows(usize);

impl<'a> Dimension<'a, TransformSummary> for OutputRows {
    fn zero((): ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &'a TransformSummary, (): ()) {
        self.0 += summary.output.lines;
    }
}

/// 变换树输入空间的字节偏移（组合文本坐标）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
struct InputOffset(MultiBufferOffset);

impl InputOffset {
    const fn get(self) -> usize {
        self.0.get()
    }
}

impl<'a> Dimension<'a, TransformSummary> for InputOffset {
    fn zero((): ()) -> Self {
        Self(MultiBufferOffset::ZERO)
    }

    fn add_summary(&mut self, summary: &'a TransformSummary, (): ()) {
        self.0 = MultiBufferOffset::new(self.0.get() + summary.input.len);
    }
}

/// Fold 层的输出字节偏移；本层坐标。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FoldOffset(MultiBufferOffset);

impl FoldOffset {
    pub(crate) const fn new(offset: MultiBufferOffset) -> Self {
        Self(offset)
    }

    pub(crate) const fn get(self) -> usize {
        self.0.get()
    }

    /// 输出字节偏移 → 输出点。
    pub(crate) fn to_point(self, snapshot: &FoldSnapshot) -> FoldPoint {
        let (start, _, item) = snapshot
            .transforms
            .find::<Dimensions<FoldOffset, TransformSummary>, _>((), &self, TreeBias::Right);
        let overshoot = self.0.get() - start.0.get();
        if item.is_none_or(Transform::is_fold) {
            // 占位符段没有真实输入点：列按占位符文本内字节计。
            FoldPoint::new(start.1.output.lines, overshoot)
        } else {
            let input_offset = MultiBufferOffset::new(start.1.input.len + overshoot);
            // 合成换行等位置的列查询可能没有对应行起点；行号由 byte_to_line 给出，列尽力而为。
            let input_line = snapshot
                .input
                .byte_to_line(input_offset)
                .expect("同构段内的输出偏移必须能映射回输入行");
            let column = snapshot
                .input
                .line_start_byte(input_line)
                .map(|line_start| input_offset.get() - line_start.get())
                .unwrap_or(0);
            FoldPoint::new(
                start.1.output.lines + input_line.get() - start.1.input.lines,
                column,
            )
        }
    }
}

impl<'a> Dimension<'a, TransformSummary> for FoldOffset {
    fn zero((): ()) -> Self {
        Self(MultiBufferOffset::ZERO)
    }

    fn add_summary(&mut self, summary: &'a TransformSummary, (): ()) {
        self.0 = MultiBufferOffset::new(self.0.get() + summary.output.len);
    }
}

/// Fold 层的输出点（行 + 列）；本层坐标。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FoldPoint {
    row: usize,
    column: usize,
}

impl FoldPoint {
    pub(crate) const fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }

    pub(crate) const fn row(self) -> usize {
        self.row
    }

    pub(crate) const fn column(self) -> usize {
        self.column
    }

    /// 输出点 → 输出字节偏移。
    ///
    /// 行首由可见逻辑行的行首映射确定；行内列按输出字节追加。
    pub(crate) fn to_offset(self, snapshot: &FoldSnapshot) -> FoldOffset {
        let start = snapshot.row_start_offset_inner(self.row);
        if self.column == 0 {
            start
        } else {
            FoldOffset::new(MultiBufferOffset::new(start.get() + self.column))
        }
    }
}

impl<'a> Dimension<'a, TransformSummary> for FoldPoint {
    fn zero((): ()) -> Self {
        Self::new(0, 0)
    }

    fn add_summary(&mut self, summary: &'a TransformSummary, (): ()) {
        self.row += summary.output.lines;
    }
}

/// Fold 层的本层编辑：输出字节区间在旧/新投影空间中的替换。
///
/// old 落在旧投影空间，new 落在新投影空间。
pub(super) type FoldEdit = ProjectionEdit<FoldOffset>;

/// 一段组合文本坐标的编辑，供 fold 拓扑定位使用。
type FoldBufferEdit = ProjectionEdit<MultiBufferOffset>;

/// 折叠的稳定身份对应的范围与实测宽度；宽度由渲染层回写。
#[derive(Debug, Clone)]
struct FoldMetadata {
    range: FoldRange,
    width: Option<Pixels>,
}

#[derive(Debug, Clone)]
pub(crate) struct FoldSnapshot {
    /// 下层组合文本快照：fold 拓扑工作在其上，外部文本可被折叠。
    input: MultiBufferSnapshot,
    folds: SumTree<Fold>,
    transforms: SumTree<Transform>,
    fold_metadata_by_id: BTreeMap<FoldId, FoldMetadata>,
    version: u64,
}

impl FoldSnapshot {
    pub(super) fn buffer_snapshot(&self) -> &MultiBufferSnapshot {
        &self.input
    }

    pub(super) const fn version(&self) -> u64 {
        self.version
    }

    /// 折叠元素上一帧的实测宽度；由渲染层回写。
    fn fold_width(&self, id: ChunkRendererId) -> Option<Pixels> {
        let ChunkRendererId::Fold(fold_id) = id;
        self.fold_metadata_by_id.get(&fold_id)?.width
    }

    /// 投影行数；行数是换行数加一，与下层 MultiBufferSnapshot::line_count 同语义。
    pub(super) fn line_count(&self) -> usize {
        self.transforms.summary().output.lines + 1
    }

    #[cfg(test)]
    fn logical_line_count(&self) -> usize {
        self.transforms.summary().input.lines + 1
    }

    /// 折叠输出文本的最后一个点。
    /// 该点是 Tab 层配置变化时整层失效范围的边界。
    pub(crate) fn max_point(&self) -> FoldPoint {
        FoldOffset::new(MultiBufferOffset::new(self.transforms.summary().output.len)).to_point(self)
    }

    /// 按当前输入快照解析所有仍有效的折叠（跳过锚点退出投影或范围退化的项）。
    ///
    /// 与 Zed 相同：折叠树跨文本编辑保持权威，不在同步时重建；解析结果随查询即时得出。
    fn resolved_folds(&self) -> impl Iterator<Item = ResolvedFold> + '_ {
        self.folds.iter().filter_map(|fold| {
            let folded = fold.resolve(&self.input)?;
            (!folded.text_range.is_empty()).then_some(folded)
        })
    }

    /// 覆盖该字节偏移的最外层折叠变换及其占位符字符数。
    ///
    /// 与 Zed FoldSnapshot::placeholder_range_at 相同，直接查询变换树，不维护跨快照的解析缓存。
    fn covering_fold(
        &self,
        offset: MultiBufferOffset,
    ) -> Option<(MultiBufferOffset, MultiBufferOffset, usize)> {
        let (start, _, item) = self
            .transforms
            .find::<Dimensions<InputOffset, FoldOffset>, _>(
                (),
                &InputOffset(offset),
                TreeBias::Right,
            );
        let transform = item?;
        let placeholder = transform.placeholder.as_ref()?;
        Some((
            start.0.0,
            MultiBufferOffset::new(start.0.get() + transform.summary.input.len),
            placeholder.text.chars().count(),
        ))
    }

    /// 覆盖该字节偏移的最外层折叠的隐藏范围（入口行换行符到闭合括号前）；无则 None。
    pub(crate) fn fold_range_covering_offset(
        &self,
        offset: MultiBufferOffset,
    ) -> Option<(MultiBufferOffset, MultiBufferOffset)> {
        self.covering_fold(offset)
            .map(|(start, end, _)| (start, end))
    }

    /// 折叠入口行（合并行占位符的挂靠行；无隐藏行的 fold 不计）。
    pub(crate) fn fold_anchor_lines(&self) -> Vec<Line> {
        self.resolved_folds()
            .filter_map(|fold| (fold.line_span.0 < fold.line_span.1).then_some(fold.line_span.0))
            .collect()
    }

    /// 指定源行范围内的折叠入口行。
    ///
    /// 只保留未被外层折叠遮蔽的入口行；直接按当前快照解析权威折叠树，不维护行索引缓存。
    pub(super) fn fold_anchor_lines_in_range(&self, line_range: Range<Line>) -> Vec<Line> {
        let mut active_ends: Vec<usize> = Vec::new();
        let mut lines = Vec::new();
        for fold in self.folds.iter() {
            let Some(fold) = fold.resolve(&self.input) else {
                continue;
            };
            if fold.text_range.is_empty() {
                continue;
            }
            let start = fold.text_range.start().get();
            let end = fold.text_range.end().get();
            active_ends.retain(|active_end| *active_end >= start);
            if fold.line_span.0 < fold.line_span.1
                && active_ends.is_empty()
                && line_range.contains(&fold.line_span.0)
            {
                lines.push(fold.line_span.0);
            }
            active_ends.push(end);
        }
        lines
    }

    /// 输出行 → 行首输出偏移。
    pub(super) fn row_start_offset(&self, row: ProjectedLineIndex) -> FoldOffset {
        FoldPoint::new(row.get(), 0).to_offset(self)
    }

    /// 输出行行首的输出偏移。
    ///
    /// 行首是可见逻辑行的行首经输入到输出映射得到的位置；不能用 FoldPoint 维度 seek，
    /// 因为占位符变换的输出行数为零，会被右偏 seek 跳过。
    fn row_start_offset_inner(&self, row: usize) -> FoldOffset {
        if row == 0 {
            return FoldOffset::new(MultiBufferOffset::ZERO);
        }
        let total = self.transforms.summary().output.len;
        let Some(text) = self.projected_line_kind(ProjectedLineIndex::new(row)) else {
            return FoldOffset::new(MultiBufferOffset::new(total));
        };
        match self.input.line_start_byte(text.logical_line()) {
            Ok(offset) => self.input_to_output_offset(offset),
            Err(_) => FoldOffset::new(MultiBufferOffset::new(total)),
        }
    }

    /// 输出偏移所在变换的输入字节范围；占位符给出折叠输入起点与终点。
    pub(super) fn input_range_at_output(
        &self,
        offset: FoldOffset,
    ) -> (MultiBufferOffset, MultiBufferOffset) {
        let (start, _, item) = self
            .transforms
            .find::<Dimensions<FoldOffset, InputOffset>, _>((), &offset, TreeBias::Right);
        let input_start = MultiBufferOffset::new(start.1.get());
        let input_end =
            MultiBufferOffset::new(input_start.get() + item.map_or(0, |t| t.summary.input.len));
        (input_start, input_end)
    }

    fn row_for_output_offset(&self, offset: MultiBufferOffset) -> usize {
        FoldOffset::new(offset).to_point(self).row()
    }

    /// 输出字节偏移 → 输入字节偏移；占位符内的偏移吸附到折叠输入起点。
    fn output_to_input_offset(&self, offset: FoldOffset) -> MultiBufferOffset {
        let (start, _, item) = self
            .transforms
            .find::<Dimensions<FoldOffset, InputOffset>, _>((), &offset, TreeBias::Right);
        if item.is_none_or(Transform::is_fold) {
            MultiBufferOffset::new(start.1.get())
        } else {
            MultiBufferOffset::new(start.1.get() + offset.get() - start.0.get())
        }
    }

    /// 输入字节偏移 → 输出字节偏移；折叠内部的偏移吸附到占位符起点。
    fn input_to_output_offset(&self, offset: MultiBufferOffset) -> FoldOffset {
        let (start, _, item) = self
            .transforms
            .find::<Dimensions<InputOffset, FoldOffset>, _>(
                (),
                &InputOffset(offset),
                TreeBias::Right,
            );
        if item.is_some_and(Transform::is_fold) {
            start.1
        } else {
            FoldOffset::new(MultiBufferOffset::new(
                start.1.get() + offset.get() - start.0.get(),
            ))
        }
    }

    /// 逻辑点 → 投影点（列在合并行字符空间中）。
    pub(super) fn logical_to_projected_point(
        &self,
        point: LogicalPoint,
        bias: FoldBias,
    ) -> DisplayMapResult<ProjectedPoint> {
        let input_offset = self.input.position_to_byte(point.into_position())?;
        if let Some((start, end, placeholder_chars)) = self.covering_fold(input_offset)
            && input_offset > start
            && input_offset < end
        {
            let output_offset = self.input_to_output_offset(start);
            let row = self.row_for_output_offset(output_offset.0);
            let projected_row = ProjectedLineIndex::new(row);
            let base = self.char_column_in_row(projected_row, output_offset.get());
            let column = match bias {
                FoldBias::Left => base,
                FoldBias::Right => base + placeholder_chars,
            };
            return Ok(ProjectedPoint::new(
                projected_row,
                LogicalColumn::new(column),
            ));
        }
        let output_offset = self.input_to_output_offset(input_offset);
        let row = self.row_for_output_offset(output_offset.0);
        let projected_row = ProjectedLineIndex::new(row);
        if !self.is_fold_row(projected_row) {
            return Ok(ProjectedPoint::new(projected_row, point.column()));
        }
        let column = self.char_column_in_row(projected_row, output_offset.get());
        Ok(ProjectedPoint::new(
            projected_row,
            LogicalColumn::new(column),
        ))
    }

    /// 投影行 → 是否包含折叠占位符（合并行）。
    pub(crate) fn is_fold_row(&self, row: ProjectedLineIndex) -> bool {
        if row.get() >= self.line_count() {
            return false;
        }
        let row_start = self.row_start_offset(row).get();
        let row_end = self
            .row_start_offset(ProjectedLineIndex::new(row.get() + 1))
            .get();
        let mut cursor = self.transforms.cursor::<FoldOffset>(());
        cursor.seek(
            &FoldOffset::new(MultiBufferOffset::new(row_start)),
            TreeBias::Right,
        );
        while let Some(transform) = cursor.item() {
            let start = cursor.start().0.get();
            if start >= row_end {
                break;
            }
            let end = start + transform.summary.output.len;
            if transform.is_fold() && end > row_start && start < row_end {
                return true;
            }
            cursor.next();
        }
        false
    }

    /// 投影行 → 折叠合并行的入口流行号。
    pub(crate) fn fold_row_anchor_stream_line(&self, row: ProjectedLineIndex) -> Option<Line> {
        if !self.is_fold_row(row) {
            return None;
        }
        let output_offset = self.row_start_offset(row);
        let input_offset = self.output_to_input_offset(output_offset);
        self.input.byte_to_line(input_offset).ok()
    }

    /// 投影行 → 段表（输出行内容字节空间的切分，文本顺序）。
    ///
    /// 段表由变换树推导：同构变换给出源行文本段，折叠变换给出占位符段。
    /// 单行 shaping 的文本段不含行终止符，因此这里裁剪掉行尾终止符。
    pub(crate) fn fold_row_segments(&self, row: ProjectedLineIndex) -> Option<Vec<FoldRowSegment>> {
        self.row_segments(row, true)
    }

    /// 投影行 → 段表；trim_terminator 为 true 时裁掉行尾终止符（shaping 输入）。
    fn row_segments(
        &self,
        row: ProjectedLineIndex,
        trim_terminator: bool,
    ) -> Option<Vec<FoldRowSegment>> {
        if row.get() >= self.line_count() {
            return None;
        }
        let row_start = self.row_start_offset(row).get();
        let row_end = self
            .row_start_offset(ProjectedLineIndex::new(row.get() + 1))
            .get();
        let mut segments = Vec::new();
        let mut has_placeholder = false;
        let mut cursor = self
            .transforms
            .cursor::<Dimensions<FoldOffset, InputOffset>>(());
        cursor.seek(
            &FoldOffset::new(MultiBufferOffset::new(row_start)),
            TreeBias::Right,
        );
        while let Some(transform) = cursor.item() {
            let transform_output_start = cursor.start().0.get();
            if transform_output_start >= row_end {
                break;
            }
            let transform_input_start = cursor.start().1.get();
            let transform_output_end = transform_output_start + transform.summary.output.len;
            let seg_start = transform_output_start.max(row_start);
            let seg_end = transform_output_end.min(row_end);
            if seg_end > seg_start {
                let merged_range = (seg_start - row_start)..(seg_end - row_start);
                match &transform.placeholder {
                    Some(placeholder) => {
                        has_placeholder = true;
                        segments.push(FoldRowSegment {
                            merged_range,
                            kind: FoldRowSegmentKind::Placeholder {
                                text: placeholder.text.clone(),
                                renderer: placeholder.renderer.clone(),
                            },
                        });
                    }
                    None => {
                        let input_start =
                            transform_input_start + (seg_start - transform_output_start);
                        let input_end = transform_input_start + (seg_end - transform_output_start);
                        let stream_line = self
                            .input
                            .byte_to_line(MultiBufferOffset::new(input_start))
                            .ok()?;
                        let line_start = self.input.line_start_byte(stream_line).ok()?.get();
                        segments.push(FoldRowSegment {
                            merged_range,
                            kind: FoldRowSegmentKind::Text {
                                stream_line,
                                projected_range: (input_start - line_start)
                                    ..(input_end - line_start),
                            },
                        });
                    }
                }
            }
            cursor.next();
        }
        if !has_placeholder {
            return None;
        }
        if trim_terminator {
            trim_last_terminator(&mut segments, &self.input);
        }
        Some(segments)
    }

    /// 投影行 → 行文本；合并行为各段的拼接（含行尾换行符）。
    pub(crate) fn row_text(&self, row: ProjectedLineIndex) -> Option<Cow<'_, str>> {
        if row.get() >= self.line_count() {
            return None;
        }
        let Some(segments) = self.row_segments(row, false) else {
            let line = self.projected_line_kind(row)?.logical_line();
            return self.input.line_text(line);
        };
        let mut text = String::new();
        for segment in &segments {
            match &segment.kind {
                FoldRowSegmentKind::Text {
                    stream_line,
                    projected_range,
                } => {
                    let line_start = self.input.line_start_byte(*stream_line).ok()?.get();
                    let range = MultiBufferRange::new(
                        MultiBufferOffset::new(line_start + projected_range.start),
                        MultiBufferOffset::new(line_start + projected_range.end),
                    )
                    .ok()?;
                    text.push_str(&self.input.text_for_range(range).ok()?);
                }
                FoldRowSegmentKind::Placeholder {
                    text: placeholder, ..
                } => text.push_str(placeholder),
            }
        }
        Some(Cow::Owned(text))
    }

    /// 投影行内输出偏移之前的字符列。
    fn char_column_in_row(&self, row: ProjectedLineIndex, output_offset: usize) -> usize {
        let Some(segments) = self.fold_row_segments(row) else {
            return 0;
        };
        let row_start = self.row_start_offset(row).get();
        let target = output_offset.saturating_sub(row_start);
        let mut column = 0;
        for segment in &segments {
            if target <= segment.merged_range.start {
                break;
            }
            let prefix = target.min(segment.merged_range.end) - segment.merged_range.start;
            match &segment.kind {
                FoldRowSegmentKind::Text {
                    stream_line,
                    projected_range,
                } => {
                    if let Ok(line_start) = self.input.line_start_byte(*stream_line)
                        && let Ok(range) = MultiBufferRange::new(
                            MultiBufferOffset::new(line_start.get() + projected_range.start),
                            MultiBufferOffset::new(
                                line_start.get() + projected_range.start + prefix,
                            ),
                        )
                        && let Ok(text) = self.input.text_for_range(range)
                    {
                        column += text.chars().count();
                    }
                }
                FoldRowSegmentKind::Placeholder { text, .. } => {
                    column += text
                        .char_indices()
                        .take_while(|(index, _)| *index < prefix)
                        .count();
                }
            }
            if target <= segment.merged_range.end {
                break;
            }
        }
        column
    }

    /// 投影行 → 对应逻辑行。
    ///
    /// 用左偏 seek：占位符变换输出行数为零，行首必须落在其前方可见行的变换上。
    pub(crate) fn projected_line_kind(&self, index: ProjectedLineIndex) -> Option<TextLine> {
        if index.get() >= self.line_count() {
            return None;
        }
        let (start, _, _) = self
            .transforms
            .find::<Dimensions<OutputRows, InputLines>, _>(
                (),
                &OutputRows(index.get()),
                TreeBias::Left,
            );
        let logical = start.1.0 + (index.get() - start.0.0);
        Some(TextLine::new(Line::new(logical)))
    }

    /// 投影行的内容来源：文本行及其 buffer 行来源。
    pub(super) fn projected_kind(
        &self,
        projected: ProjectedLineIndex,
    ) -> Option<StreamProjectedKind> {
        let text = self.projected_line_kind(projected)?;
        Some(StreamProjectedKind::Text(text.logical_line()))
    }
}

/// 投影行（fold 输出）的内容来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamProjectedKind {
    /// 可见文本行及其 buffer 行来源。
    Text(Line),
}

#[derive(Debug, Clone)]
pub(super) struct FoldMap {
    snapshot: FoldSnapshot,
    next_fold_id: FoldId,
}

impl FoldMap {
    pub(super) fn new(input: MultiBufferSnapshot) -> (Self, FoldSnapshot) {
        let summary = text_summary_for_range(&input, MultiBufferOffset::ZERO, input.len_bytes());
        let transforms = SumTree::from_iter(
            [Transform {
                summary: TransformSummary {
                    input: summary,
                    output: summary,
                },
                placeholder: None,
            }],
            (),
        );
        let folds = SumTree::new(&input);
        let snapshot = FoldSnapshot {
            input,
            folds,
            transforms,
            fold_metadata_by_id: BTreeMap::new(),
            version: 0,
        };
        (
            Self {
                snapshot: snapshot.clone(),
                next_fold_id: FoldId::INITIAL,
            },
            snapshot,
        )
    }

    pub(super) fn snapshot(&self) -> &FoldSnapshot {
        &self.snapshot
    }

    pub(super) fn read(
        &mut self,
        input: MultiBufferSnapshot,
        buffer_edits: Vec<FoldBufferEdit>,
    ) -> (FoldSnapshot, Vec<FoldEdit>) {
        let edits = self.sync(input, buffer_edits);
        (self.snapshot.clone(), edits)
    }

    pub(super) fn write(&mut self) -> FoldMapWriter<'_> {
        FoldMapWriter(self)
    }

    /// 用下层文本编辑推进变换树，并把每条编辑映射到本层输出字节区间。
    ///
    /// 对齐 Zed FoldMap::sync：重建受编辑影响的变换，未变区域直接搬运旧变换；
    /// 每条编辑经旧/新变换树映射，端点落在折叠内时吸附到折叠边界，产出精确的 FoldEdit。
    fn sync(
        &mut self,
        input: MultiBufferSnapshot,
        buffer_edits: Vec<FoldBufferEdit>,
    ) -> Vec<FoldEdit> {
        if buffer_edits.is_empty() {
            if self.snapshot.input.version() != input.version() {
                self.snapshot.version += 1;
            }
            self.snapshot.input = input;
            self.check_invariants();
            return Vec::new();
        }

        // 文本编辑可能让既有折叠的端点退出可见投影；
        // 先移除失效折叠，保持树的稳定身份顺序。
        self.drop_unresolvable_folds(&input);
        let old_transforms = std::mem::take(&mut self.snapshot.transforms);
        let mut new_transforms = SumTree::<Transform>::default();
        let mut cursor = old_transforms.cursor::<InputOffset>(());
        cursor.seek(&InputOffset(MultiBufferOffset::ZERO), TreeBias::Right);

        let mut edits_iter = buffer_edits.iter().cloned().peekable();
        while let Some(mut edit) = edits_iter.next() {
            if let Some(item) = cursor.item()
                && !item.is_fold()
            {
                new_transforms.update_last(
                    |transform| {
                        if !transform.is_fold() {
                            transform.summary.input += item.summary.input;
                            transform.summary.output += item.summary.output;
                            cursor.next();
                        }
                    },
                    (),
                );
            }

            new_transforms.append(
                cursor.slice(&InputOffset(edit.old.start), TreeBias::Left),
                (),
            );
            let snapped_old_start = cursor.start().0;
            let snap = edit.old.start.get() - snapped_old_start.get();
            edit.new.start = MultiBufferOffset::new(
                edit.new
                    .start
                    .get()
                    .checked_sub(snap)
                    .expect("fold 编辑映射不应下溢"),
            );
            edit.old.start = snapped_old_start;

            cursor.seek(&InputOffset(edit.old.end), TreeBias::Right);
            cursor.next();

            let mut delta = edit.new.end.get() as isize
                - edit.new.start.get() as isize
                - (edit.old.end.get() as isize - edit.old.start.get() as isize);
            loop {
                edit.old.end = cursor.start().0;
                let Some(next_edit) = edits_iter.peek() else {
                    break;
                };
                if next_edit.old.start > edit.old.end {
                    break;
                }
                let next_edit = edits_iter.next().unwrap();
                delta += (next_edit.new.end.get() as isize - next_edit.new.start.get() as isize)
                    - (next_edit.old.end.get() as isize - next_edit.old.start.get() as isize);
                if next_edit.old.end >= edit.old.end {
                    edit.old.end = next_edit.old.end;
                    cursor.seek(&InputOffset(next_edit.old.end), TreeBias::Right);
                    cursor.next();
                }
            }
            let old_len = edit.old.end.get() - edit.old.start.get();
            edit.new.end = MultiBufferOffset::new(
                ((edit.new.start.get() + old_len) as isize + delta).max(0) as usize,
            );

            let start_anchor = input.anchor_at(edit.new.start, Affinity::Before);
            let mut folds_cursor = self.snapshot.folds.cursor::<FoldRange>(&input);
            folds_cursor.seek(
                &FoldRange(start_anchor..MultiBufferAnchor::Max),
                TreeBias::Left,
            );
            let mut folds = std::iter::from_fn(|| {
                loop {
                    let fold = folds_cursor.item().cloned()?;
                    folds_cursor.next();
                    let Some(fold) = fold.resolve(&input) else {
                        continue;
                    };
                    let range = fold.text_range;
                    if !range.is_empty() {
                        return Some((fold, range));
                    }
                }
            })
            .peekable();

            while folds
                .peek()
                .is_some_and(|(_, fold_range)| fold_range.start() < edit.new.end)
            {
                let (fold, fold_range) = folds.next().expect("peek 后必须存在折叠");
                let sum = new_transforms.summary();
                if fold_range.start().get() < sum.input.len {
                    // 起点锚点已删除或收敛到前缀内：该折叠的变换已由前缀保留，跳过重建。
                    continue;
                }
                let mut merge_end = fold_range.end();
                while let Some((next, next_range)) = folds.peek() {
                    let can_merge = next_range.start() < merge_end
                        || (next_range.start() == merge_end
                            && fold.fold.placeholder.merge_adjacent
                            && next.fold.placeholder.merge_adjacent);
                    if !can_merge {
                        break;
                    }
                    merge_end = merge_end.max(next_range.end());
                    folds.next();
                }

                if merge_end > edit.new.end {
                    // 折叠合并的重建前沿属于本次编辑的失效区间：
                    // 让旧变换游标和新输入前沿一起越过它，保证 suffix 从重建区之后开始，而不是从 edit.old.end 的旧边界开始。
                    let old_merge_end = MultiBufferOffset::new(
                        edit.old.end.get() + (merge_end.get() - edit.new.end.get()),
                    );
                    cursor.seek(&InputOffset(old_merge_end), TreeBias::Right);
                    // 只在 old_merge_end 落在旧变换内部时才越过它；
                    // 正好落在边界时 suffix 就从这里开始。
                    if cursor.start().0 < old_merge_end {
                        cursor.next();
                    }
                    let mut advanced_old_end = cursor.start().0;
                    // 本次重建区已覆盖的后续编辑：其变化已由当前快照物化，直接消费，避免游标被越过后再回退；
                    // 同时累计它们的净编辑长度，把重建前沿在新坐标中一起推进。
                    let mut consumed_delta = 0isize;
                    while edits_iter
                        .peek()
                        .is_some_and(|next| next.old.start < advanced_old_end)
                    {
                        let next = edits_iter.next().unwrap();
                        consumed_delta += (next.new.end.get() as isize
                            - next.new.start.get() as isize)
                            - (next.old.end.get() as isize - next.old.start.get() as isize);
                        if next.old.end > advanced_old_end {
                            let target = next.old.end;
                            cursor.seek(&InputOffset(target), TreeBias::Right);
                            if cursor.start().0 < target {
                                cursor.next();
                            }
                            advanced_old_end = cursor.start().0;
                        }
                    }
                    let old_extension = advanced_old_end.get() - edit.old.end.get();
                    edit.old.end = advanced_old_end;
                    edit.new.end = MultiBufferOffset::new(
                        (edit.new.end.get() as isize + old_extension as isize + consumed_delta)
                            .max(0) as usize,
                    );
                }

                let sum = new_transforms.summary();
                if fold_range.start().get() > sum.input.len {
                    let text_summary = text_summary_for_range(
                        &input,
                        MultiBufferOffset::new(sum.input.len),
                        fold_range.start(),
                    );
                    push_isomorphic(&mut new_transforms, text_summary);
                }
                if merge_end > fold_range.start() {
                    let placeholder_text: Arc<str> = Arc::from(fold.fold.placeholder.text());
                    let input_summary =
                        text_summary_for_range(&input, fold_range.start(), merge_end);
                    let output_summary = text_summary_of_str(&placeholder_text);
                    new_transforms.push(
                        Transform {
                            summary: TransformSummary {
                                input: input_summary,
                                output: output_summary,
                            },
                            placeholder: Some(TransformPlaceholder {
                                text: placeholder_text,
                                renderer: ChunkRenderer {
                                    id: ChunkRendererId::Fold(fold.fold.id),
                                    render: {
                                        let render = Arc::clone(&fold.fold.placeholder.render);
                                        let fold_id = fold.fold.id;
                                        let fold_anchor_range = fold.fold.range.0.clone();
                                        Arc::new(move |cx: &mut App| {
                                            render(fold_id, fold_anchor_range.clone(), cx)
                                        })
                                    },
                                    constrain_width: fold.fold.placeholder.constrain_width,
                                    measured_width: self
                                        .snapshot
                                        .fold_width(ChunkRendererId::Fold(fold.fold.id)),
                                },
                            }),
                        },
                        (),
                    );
                }
            }

            let sum = new_transforms.summary();
            if MultiBufferOffset::new(sum.input.len) < edit.new.end {
                let text_summary = text_summary_for_range(
                    &input,
                    MultiBufferOffset::new(sum.input.len),
                    edit.new.end,
                );
                push_isomorphic(&mut new_transforms, text_summary);
            }
        }

        let suffix = cursor.suffix();
        for transform in suffix.iter() {
            if transform.is_fold() {
                new_transforms.push(transform.clone(), ());
            } else {
                push_isomorphic(&mut new_transforms, transform.summary.input);
            }
        }
        if new_transforms.is_empty() {
            let summary =
                text_summary_for_range(&input, MultiBufferOffset::ZERO, input.len_bytes());
            push_isomorphic(&mut new_transforms, summary);
        }

        drop(cursor);

        let fold_edits = {
            let mut old_transforms =
                old_transforms.cursor::<Dimensions<InputOffset, FoldOffset>>(());
            let mut new_transforms_cursor =
                new_transforms.cursor::<Dimensions<InputOffset, FoldOffset>>(());
            let mut fold_edits = Vec::with_capacity(buffer_edits.len());
            for edit in &buffer_edits {
                let old_range =
                    map_input_range_to_output(&mut old_transforms, edit.old.start, edit.old.end);
                let new_range = map_input_range_to_output(
                    &mut new_transforms_cursor,
                    edit.new.start,
                    edit.new.end,
                );
                fold_edits.push(FoldEdit {
                    old: FoldOffset::new(MultiBufferOffset::new(old_range.0))
                        ..FoldOffset::new(MultiBufferOffset::new(old_range.1)),
                    new: FoldOffset::new(MultiBufferOffset::new(new_range.0))
                        ..FoldOffset::new(MultiBufferOffset::new(new_range.1)),
                });
            }
            consolidate_fold_edits(fold_edits)
        };

        self.snapshot.transforms = new_transforms;
        self.snapshot.input = input;
        self.snapshot.version += 1;
        self.check_invariants();
        fold_edits
    }

    /// 文本编辑后移除已失效的折叠身份：端点退出可见投影或范围退化。
    ///
    /// 折叠身份跨编辑保留；稳定 Anchor 顺序由 `MultiBufferAnchor::cmp` 保证，这里只删除失效项，不重排树序。
    /// 没有折叠或全部仍有效时不做工作。
    fn drop_unresolvable_folds(&mut self, input: &MultiBufferSnapshot) {
        let total = self.snapshot.folds.summary().count;
        if total == 0 {
            return;
        }
        let mut resolved: Vec<Fold> = Vec::with_capacity(total);
        for fold in self.snapshot.folds.iter() {
            let Some(folded) = fold.resolve(input) else {
                continue;
            };
            if folded.text_range.is_empty() {
                continue;
            }
            let start_is_projected = matches!(
                input.projected_anchor_offset(&fold.range.start),
                Ok(Some(_))
            );
            let end_is_projected =
                matches!(input.projected_anchor_offset(&fold.range.end), Ok(Some(_)));
            if start_is_projected && end_is_projected {
                resolved.push(folded.fold);
            }
        }
        if resolved.len() == total {
            return;
        }
        let live: BTreeSet<FoldId> = resolved.iter().map(|fold| fold.id).collect();
        self.snapshot
            .fold_metadata_by_id
            .retain(|id, _| live.contains(id));
        self.snapshot.folds = SumTree::from_iter(resolved, input);
    }

    fn check_invariants(&self) {
        #[cfg(test)]
        {
            assert_eq!(
                self.snapshot.transforms.summary().input.len,
                self.snapshot.input.len_bytes().get(),
                "折叠变换树输入必须精确覆盖下层快照"
            );

            let mut previous_isomorphic = false;
            for transform in self.snapshot.transforms.iter() {
                assert!(
                    transform.is_fold() || !previous_isomorphic,
                    "折叠变换树不得包含相邻同构段"
                );
                previous_isomorphic = !transform.is_fold();
            }
        }
    }
}

/// 把输入字节区间经变换树映射到输出字节区间；端点落在折叠内时吸附到折叠边界。
fn map_input_range_to_output(
    cursor: &mut sum_tree::Cursor<'_, 'static, Transform, Dimensions<InputOffset, FoldOffset>>,
    start: MultiBufferOffset,
    end: MultiBufferOffset,
) -> (usize, usize) {
    let mut start = start;
    cursor.seek(&InputOffset(start), TreeBias::Left);
    if cursor.item().is_some_and(Transform::is_fold) {
        start = cursor.start().0.0;
    }
    let output_start = cursor.start().1.get() + (start.get() - cursor.start().0.get());

    let mut end = end;
    cursor.seek_forward(&InputOffset(end), TreeBias::Right);
    if cursor.item().is_some_and(Transform::is_fold) {
        cursor.next();
        end = cursor.start().0.0;
    }
    let output_end = cursor.start().1.get() + (end.get() - cursor.start().0.get());
    (output_start, output_end)
}

fn text_summary_for_range(
    input: &MultiBufferSnapshot,
    start: MultiBufferOffset,
    end: MultiBufferOffset,
) -> MBTextSummary {
    input
        .text_summary_for_range(MultiBufferRange::new(start, end).expect("折叠变换区间必须有序"))
        .expect("折叠变换区间必须落在快照内")
}

fn text_summary_of_str(text: &str) -> MBTextSummary {
    MBTextSummary {
        len: text.len(),
        chars: text.chars().count(),
        len_utf16: text.chars().map(char::len_utf16).sum(),
        lines: text.bytes().filter(|byte| *byte == b'\n').count(),
    }
}

/// 裁掉输出行最后一段的行终止符，使单行 shaping 输入不含终止符。
fn trim_last_terminator(segments: &mut Vec<FoldRowSegment>, input: &MultiBufferSnapshot) {
    let Some(index) = segments.len().checked_sub(1) else {
        return;
    };
    let FoldRowSegmentKind::Text {
        stream_line,
        projected_range,
    } = &segments[index].kind
    else {
        return;
    };
    let Ok(line_start) = input.line_start_byte(*stream_line) else {
        return;
    };
    let Some(content) = input.line_content_byte_range(*stream_line) else {
        return;
    };
    let content_end = content.end.get().saturating_sub(line_start.get());
    let (start, end) = (projected_range.start, projected_range.end);
    if end <= content_end {
        return;
    }
    if start >= content_end {
        segments.pop();
        return;
    }
    let trimmed = end - content_end;
    if let FoldRowSegmentKind::Text {
        projected_range, ..
    } = &mut segments[index].kind
    {
        projected_range.end = content_end;
    }
    segments[index].merged_range.end = segments[index].merged_range.end.saturating_sub(trimmed);
}

fn push_isomorphic(transforms: &mut SumTree<Transform>, summary: MBTextSummary) {
    let mut did_merge = false;
    transforms.update_last(
        |last| {
            if !last.is_fold() {
                last.summary.input += summary;
                last.summary.output += summary;
                did_merge = true;
            }
        },
        (),
    );
    if !did_merge {
        transforms.push(
            Transform {
                summary: TransformSummary {
                    input: summary,
                    output: summary,
                },
                placeholder: None,
            },
            (),
        );
    }
}

fn consolidate_fold_edits(mut edits: Vec<FoldEdit>) -> Vec<FoldEdit> {
    edits.sort_unstable_by(|a, b| {
        a.old
            .start
            .cmp(&b.old.start)
            .then_with(|| b.old.end.cmp(&a.old.end))
    });
    let mut merged: Vec<FoldEdit> = Vec::with_capacity(edits.len());
    for edit in edits {
        match merged.last_mut() {
            Some(prev) if prev.old.end >= edit.old.start => {
                prev.old.end = prev.old.end.max(edit.old.end);
                prev.new.start = prev.new.start.min(edit.new.start);
                prev.new.end = prev.new.end.max(edit.new.end);
            }
            _ => merged.push(edit),
        }
    }
    merged
}

fn consolidate_fold_buffer_edits(mut edits: Vec<FoldBufferEdit>) -> Vec<FoldBufferEdit> {
    edits.sort_unstable_by(|a, b| {
        a.old
            .start
            .cmp(&b.old.start)
            .then_with(|| b.old.end.cmp(&a.old.end))
    });
    let mut merged: Vec<FoldBufferEdit> = Vec::with_capacity(edits.len());
    for edit in edits {
        match merged.last_mut() {
            Some(prev) if prev.old.end >= edit.old.start => {
                prev.old.end = prev.old.end.max(edit.old.end);
                prev.new.start = prev.new.start.min(edit.new.start);
                prev.new.end = prev.new.end.max(edit.new.end);
            }
            _ => merged.push(edit),
        }
    }
    merged
}

pub(super) struct FoldMapWriter<'a>(&'a mut FoldMap);

impl FoldMapWriter<'_> {
    /// 展开与行范围交叠的全部折叠（半开区间）。
    pub(super) fn unfold_lines(
        &mut self,
        line_range: LineRange,
    ) -> DisplayMapResult<(FoldSnapshot, Vec<FoldEdit>)> {
        let snapshot = self.0.snapshot.buffer_snapshot();
        let ids: BTreeSet<_> = self
            .0
            .snapshot
            .folds
            .iter()
            .filter_map(|fold| {
                let folded = fold.resolve(snapshot)?;
                (folded.line_span.0.get() < line_range.end().get()
                    && folded.line_span.1.get() >= line_range.start().get())
                .then_some(fold.id)
            })
            .collect();
        Ok(self.unfold_ids(ids))
    }

    pub(super) fn fold(
        &mut self,
        range: Range<MultiBufferAnchor>,
        placeholder: FoldPlaceholder,
    ) -> DisplayMapResult<(FoldSnapshot, Vec<FoldEdit>)> {
        let resolved = {
            let snapshot = self.0.snapshot.buffer_snapshot();
            let start = snapshot
                .projected_anchor_offset(&range.start)
                .map_err(|_| FoldError::UnresolvableAnchor)?
                .ok_or(FoldError::UnresolvableAnchor)?;
            let end = snapshot
                .projected_anchor_offset(&range.end)
                .map_err(|_| FoldError::UnresolvableAnchor)?
                .ok_or(FoldError::UnresolvableAnchor)?;
            MultiBufferRange::new(start, end)?
        };
        if resolved.is_empty() {
            return Err(FoldError::EmptyRange { range: resolved }.into());
        }
        let current_snapshot = self.0.snapshot.buffer_snapshot().clone();
        if self.0.snapshot.folds.iter().any(|fold| {
            fold.resolve(&current_snapshot)
                .is_some_and(|fold| fold.text_range == resolved)
        }) {
            return Ok((self.0.snapshot.clone(), Vec::new()));
        }
        for fold in self.0.snapshot.folds.iter() {
            let Some(existing) = fold.resolve(&current_snapshot) else {
                continue;
            };
            if !ranges_disjoint_or_nested(existing.text_range, resolved) {
                return Err(FoldError::OverlapWithoutNesting {
                    existing: existing.text_range,
                    candidate: resolved,
                }
                .into());
            }
        }
        let id = self.0.next_fold_id;
        self.0.next_fold_id = FoldId(
            self.0
                .next_fold_id
                .0
                .checked_add(1)
                .ok_or(FoldError::IdOverflow)?,
        );
        let fold =
            Fold::from_text_range(self.0.snapshot.buffer_snapshot(), id, resolved, placeholder);
        let old_folds =
            std::mem::replace(&mut self.0.snapshot.folds, SumTree::new(&current_snapshot));
        let mut cursor = old_folds.cursor::<FoldRange>(&current_snapshot);
        let mut folds = SumTree::new(&current_snapshot);
        folds.append(
            cursor.slice(&fold.range, TreeBias::Right),
            &current_snapshot,
        );
        folds.push(fold.clone(), &current_snapshot);
        folds.append(cursor.suffix(), &current_snapshot);
        drop(cursor);
        self.0.snapshot.folds = folds;
        self.0.snapshot.fold_metadata_by_id.insert(
            id,
            FoldMetadata {
                range: fold.range.clone(),
                width: None,
            },
        );
        let input = self.0.snapshot.input.clone();
        let edit = FoldBufferEdit::new(
            resolved.start()..resolved.end(),
            resolved.start()..resolved.end(),
        );
        let edits = self.0.sync(input, vec![edit]);
        Ok((self.0.snapshot.clone(), edits))
    }

    fn unfold_ids(
        &mut self,
        ids: impl IntoIterator<Item = FoldId>,
    ) -> (FoldSnapshot, Vec<FoldEdit>) {
        let ids: BTreeSet<_> = ids.into_iter().collect();
        if ids.is_empty() {
            return (self.0.snapshot.clone(), Vec::new());
        }

        let current_snapshot = self.0.snapshot.buffer_snapshot().clone();
        let selected: Vec<_> = self
            .0
            .snapshot
            .folds
            .iter()
            .filter_map(|fold| {
                let folded = fold.resolve(&current_snapshot)?;
                ids.contains(&fold.id).then_some(folded)
            })
            .collect();
        if selected.is_empty() {
            return (self.0.snapshot.clone(), Vec::new());
        }

        let edits = selected
            .iter()
            .map(|fold| {
                FoldBufferEdit::new(
                    fold.text_range.start()..fold.text_range.end(),
                    fold.text_range.start()..fold.text_range.end(),
                )
            })
            .collect();
        let indices: Vec<_> = self
            .0
            .snapshot
            .folds
            .iter()
            .enumerate()
            .filter_map(|(index, fold)| ids.contains(&fold.id).then_some(index))
            .collect();

        let old_folds =
            std::mem::replace(&mut self.0.snapshot.folds, SumTree::new(&current_snapshot));
        let mut cursor = old_folds.cursor::<FoldIndex>(&current_snapshot);
        cursor.seek(&FoldIndex(0), TreeBias::Right);
        let mut folds = SumTree::new(&current_snapshot);
        for index in indices {
            folds.append(
                cursor.slice(&FoldIndex(index), TreeBias::Right),
                &current_snapshot,
            );
            cursor.next();
        }
        folds.append(cursor.suffix(), &current_snapshot);
        drop(cursor);
        self.0.snapshot.folds = folds;
        for fold in &selected {
            self.0.snapshot.fold_metadata_by_id.remove(&fold.fold.id);
        }

        let input = self.0.snapshot.input.clone();
        let edits = self.0.sync(input, consolidate_fold_buffer_edits(edits));
        (self.0.snapshot.clone(), edits)
    }

    /// 回写渲染层实测的元素宽度；宽度变化时产生该折叠的零宽编辑并重新同步。
    ///
    /// 返回新的折叠快照与传播到上层的 FoldEdit；宽度未变化时不产生编辑，也不推进快照。
    pub(super) fn update_fold_widths(
        &mut self,
        widths: impl IntoIterator<Item = (ChunkRendererId, Pixels)>,
    ) -> (FoldSnapshot, Vec<FoldEdit>) {
        let mut edits = Vec::new();
        for (id, new_width) in widths {
            let ChunkRendererId::Fold(fold_id) = id;
            let Some(metadata) = self.0.snapshot.fold_metadata_by_id.get(&fold_id).cloned() else {
                continue;
            };
            if Some(new_width) == metadata.width {
                continue;
            }
            let Some(range) =
                resolve_fold_range(self.0.snapshot.buffer_snapshot(), &metadata.range)
            else {
                continue;
            };
            // 折叠内容未变，只有元素像素宽度变化：
            // 用零宽输入编辑让该折叠重新走 sync，刷新变换树中的 measured_width，并向上层发布该折叠的显示编辑。
            edits.push(FoldBufferEdit::new(
                range.start()..range.end(),
                range.start()..range.end(),
            ));
            self.0.snapshot.fold_metadata_by_id.insert(
                fold_id,
                FoldMetadata {
                    range: metadata.range,
                    width: Some(new_width),
                },
            );
        }
        if edits.is_empty() {
            return (self.0.snapshot.clone(), Vec::new());
        }
        let input = self.0.snapshot.input.clone();
        let edits = self.0.sync(input, edits);
        (self.0.snapshot.clone(), edits)
    }
}

fn resolve_fold_range(
    snapshot: &MultiBufferSnapshot,
    range: &FoldRange,
) -> Option<MultiBufferRange> {
    let start = snapshot.projected_anchor_offset(&range.start).ok()??;
    let end = snapshot.projected_anchor_offset(&range.end).ok()??;
    MultiBufferRange::new(start, end).ok()
}

/// 折叠范围的逻辑行跨度：起点行（anchor）与终点所在行（close）。
fn fold_line_span(
    snapshot: &MultiBufferSnapshot,
    range: MultiBufferRange,
) -> DisplayMapResult<(Line, Line)> {
    let start = snapshot.byte_to_line(range.start())?;
    let end = snapshot.byte_to_line(range.end())?;
    Ok((start, end))
}

fn ranges_disjoint_or_nested(left: MultiBufferRange, right: MultiBufferRange) -> bool {
    left.end() <= right.start()
        || right.end() <= left.start()
        || (left.start() <= right.start() && right.end() <= left.end())
        || (right.start() <= left.start() && left.end() <= right.end())
}

/// FoldSnapshot 中投影行的 0-indexed 索引。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub(crate) struct ProjectedLineIndex(usize);

impl ProjectedLineIndex {
    pub(crate) const fn new(value: usize) -> Self {
        Self(value)
    }

    pub(crate) const fn get(self) -> usize {
        self.0
    }
}

/// 可见逻辑行投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TextLine {
    logical_line: Line,
}

impl TextLine {
    pub(crate) fn new(logical_line: Line) -> Self {
        Self { logical_line }
    }

    pub(crate) fn logical_line(self) -> Line {
        self.logical_line
    }
}

/// 逻辑文档内的 (line, column) 点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(super) struct LogicalPoint {
    line: Line,
    column: LogicalColumn,
}

impl LogicalPoint {
    pub(super) const fn new(line: Line, column: LogicalColumn) -> Self {
        Self { line, column }
    }

    pub(super) const fn column(self) -> LogicalColumn {
        self.column
    }

    pub(super) fn into_position(self) -> Position {
        Position::new(self.line, self.column)
    }
}

impl From<Position> for LogicalPoint {
    fn from(position: Position) -> Self {
        Self {
            line: position.line(),
            column: position.column(),
        }
    }
}

impl From<LogicalPoint> for Position {
    fn from(point: LogicalPoint) -> Self {
        point.into_position()
    }
}

/// 投影空间内的 (projected_line, column) 点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct ProjectedPoint {
    line: ProjectedLineIndex,
    column: LogicalColumn,
}

impl ProjectedPoint {
    pub(crate) const fn new(line: ProjectedLineIndex, column: LogicalColumn) -> Self {
        Self { line, column }
    }

    pub(crate) const fn line(self) -> ProjectedLineIndex {
        self.line
    }

    pub(crate) const fn column(self) -> LogicalColumn {
        self.column
    }
}

/// 输出行文本的段：输出行字节空间的切分（只服务列与字节映射、高亮坐标域，不参与行数）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FoldRowSegment {
    /// 段在输出行文本中的投影字节范围。
    pub(super) merged_range: Range<usize>,
    pub(super) kind: FoldRowSegmentKind,
}

impl FoldRowSegment {
    pub(crate) fn merged_range(&self) -> &Range<usize> {
        &self.merged_range
    }
}

/// 输出行段的来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FoldRowSegmentKind {
    /// 行内容段：流行号与行内投影字节范围。
    Text {
        stream_line: Line,
        projected_range: Range<usize>,
    },
    /// 折叠占位符段（无源坐标）；`renderer` 描述它在显示层如何被替换为元素。
    Placeholder {
        text: Arc<str>,
        renderer: ChunkRenderer,
    },
}

/// 逻辑文档内的有序点对范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct LogicalRange {
    start: LogicalPoint,
    end: LogicalPoint,
}

impl LogicalRange {
    /// 要求 start <= end（按 line, column 字典序）。
    pub(super) fn new(start: LogicalPoint, end: LogicalPoint) -> Result<Self, CoordinateError> {
        if !is_ordered_logical(start, end) {
            return Err(CoordinateError::InvalidLineRange {
                start: start.line,
                end: end.line,
            });
        }
        Ok(Self { start, end })
    }

    pub(super) const fn start(self) -> LogicalPoint {
        self.start
    }

    pub(super) const fn end(self) -> LogicalPoint {
        self.end
    }

    pub(super) fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// 投影空间内的有序点对范围。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ProjectedRange {
    start: ProjectedPoint,
    end: ProjectedPoint,
}

impl ProjectedRange {
    /// 要求 start <= end（按 projected line, column 字典序）。
    pub(crate) fn new(start: ProjectedPoint, end: ProjectedPoint) -> Result<Self, CoordinateError> {
        if !is_ordered_projected(start, end) {
            return Err(CoordinateError::InvalidLineRange {
                start: Line::new(start.line.get()),
                end: Line::new(end.line.get()),
            });
        }
        Ok(Self { start, end })
    }

    pub(crate) const fn start(self) -> ProjectedPoint {
        self.start
    }

    pub(crate) const fn end(self) -> ProjectedPoint {
        self.end
    }
}

fn is_ordered_logical(start: LogicalPoint, end: LogicalPoint) -> bool {
    if start.line < end.line {
        return true;
    }
    if start.line == end.line {
        return start.column <= end.column;
    }
    false
}

fn is_ordered_projected(start: ProjectedPoint, end: ProjectedPoint) -> bool {
    if start.line < end.line {
        return true;
    }
    if start.line == end.line {
        return start.column <= end.column;
    }
    false
}

#[cfg(test)]
#[path = "test/fold_map_tests.rs"]
mod tests;
