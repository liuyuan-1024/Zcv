//! Editor FoldSet：同一 BufferVersion 下的折叠区间集合。
//!
//! FoldSet 维护折叠集合自身的不变量：
//! - id 单 FoldSet 内单调递增；
//! - 任意两个 fold 之间必须满足「互不相交」或「严格嵌套」，禁止部分重叠；
//! - 当通过组合 `TextPatch` 推进版本时，每条 fold 按其 `TrackedRangeUpdatePolicy` 决定保留 / 塌缩 / 失效。
//! - fold 与合并后的隐藏行段分别存入持久化摘要树，快照 clone 和局部增删共享未变化节点。
//!
//! 折叠占位符样式、投影坐标和 viewport 切片由 `projection` 模块承载，不在本文件承诺。

use std::{cmp::Reverse, collections::BTreeMap};

use gpui_sum_tree::{Bias as TreeBias, ContextLessSummary, Dimension, Item, SumTree};
use zcv_engine::{
    BufferVersion, ByteOffset, Line, LineRange, PositionMap, Snapshot, TextPatch, TextRange,
    TrackedRangeUpdatePolicy,
};

use super::{
    FoldRange, FoldRangeId, FoldRangeUpdate, HiddenRange,
    geometry::{char_range_for_line_range, fold_line_span, next_line},
    range::default_update_policy,
};
use crate::editor::display_map::error::{DisplayMapResult, FoldError};

/// 一次 toggle 操作的结果：判定是新增了 fold 还是移除了已有 fold。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldToggleOutcome {
    /// 该 range 之前不存在，已新增 fold。
    Folded(FoldRangeId),
    /// 该 range 之前存在，已移除 fold。
    Unfolded(FoldRangeId),
}

/// 同一 BufferVersion 下的折叠区间集合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldSet {
    version: BufferVersion,
    next_id: FoldRangeId,
    default_update_policy: TrackedRangeUpdatePolicy,
    ranges: SumTree<FoldRange>,
    keys_by_id: BTreeMap<FoldRangeId, FoldOrder>,
    hidden_spans: SumTree<HiddenSpan>,
    last_patch: Option<(BufferVersion, BufferVersion)>,
    last_patch_changed_hidden_spans: bool,
}

impl FoldSet {
    pub fn new(version: BufferVersion) -> Self {
        Self {
            version,
            next_id: FoldRangeId::INITIAL,
            default_update_policy: default_update_policy(),
            ranges: SumTree::new(()),
            keys_by_id: BTreeMap::new(),
            hidden_spans: SumTree::new(()),
            last_patch: None,
            last_patch_changed_hidden_spans: false,
        }
    }

    pub fn with_default_update_policy(mut self, policy: TrackedRangeUpdatePolicy) -> Self {
        self.default_update_policy = policy;
        self
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    pub fn len(&self) -> usize {
        self.ranges.summary().count
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn default_update_policy(&self) -> TrackedRangeUpdatePolicy {
        self.default_update_policy
    }

    pub fn iter(&self) -> impl Iterator<Item = &FoldRange> {
        self.ranges.iter()
    }

    pub fn get(&self, id: FoldRangeId) -> Option<&FoldRange> {
        let key = self.keys_by_id.get(&id)?;
        let (_, _, fold) = self.ranges.find::<FoldOrder, _>((), key, TreeBias::Left);
        fold.filter(|fold| fold.id() == id)
    }

    /// 折叠任意合法 byte range；若已存在精确相同的 range 则返回该 fold 的 id（幂等）。
    ///
    /// `snapshot` 必须与本 FoldSet 同版本：构造时 eager 缓存 fold 的逻辑行跨度，
    /// 让后续 `Projection::build` / 增量分类器无需再为每条 fold 做 byte→line 转换。
    pub fn fold(&mut self, snapshot: &Snapshot, range: TextRange) -> DisplayMapResult<FoldRangeId> {
        self.fold_with_policy(snapshot, range, self.default_update_policy)
    }

    pub fn fold_with_policy(
        &mut self,
        snapshot: &Snapshot,
        range: TextRange,
        update_policy: TrackedRangeUpdatePolicy,
    ) -> DisplayMapResult<FoldRangeId> {
        self.ensure_snapshot_version(snapshot)?;
        if range.is_empty() {
            return Err(FoldError::EmptyRange { range }.into());
        }

        if let Some(existing) = self.find_exact(range) {
            return Ok(existing);
        }

        self.validate_nesting(range)?;
        let line_span = fold_line_span(snapshot, range)?;
        let id = self.reserve_id()?;
        let fold = FoldRange::with_policy(id, self.version, range, update_policy, line_span);
        self.insert_fold(fold);
        self.last_patch = None;
        self.last_patch_changed_hidden_spans = false;
        self.insert_hidden_span_for_fold(fold);
        Ok(id)
    }

    /// 按 line range 折叠：line range 转换为对应 byte range 后走通用 fold 入口。
    pub fn fold_lines(
        &mut self,
        snapshot: &Snapshot,
        line_range: LineRange,
    ) -> DisplayMapResult<FoldRangeId> {
        self.ensure_snapshot_version(snapshot)?;
        let range = char_range_for_line_range(snapshot, line_range)?;
        self.fold(snapshot, range)
    }

    pub fn fold_lines_with_policy(
        &mut self,
        snapshot: &Snapshot,
        line_range: LineRange,
        update_policy: TrackedRangeUpdatePolicy,
    ) -> DisplayMapResult<FoldRangeId> {
        self.ensure_snapshot_version(snapshot)?;
        let range = char_range_for_line_range(snapshot, line_range)?;
        self.fold_with_policy(snapshot, range, update_policy)
    }

    /// 移除指定 id 的 fold。返回被移除的 FoldRange；若 id 不存在则返回 None。
    pub fn unfold(&mut self, id: FoldRangeId) -> Option<FoldRange> {
        let key = *self.keys_by_id.get(&id)?;
        let removed_before_update = self.get(id).copied()?;
        let affected_hidden_span = self
            .hidden_span_for_fold(&removed_before_update)
            .and_then(|span| self.overlapping_hidden_span(span));
        let old_ranges = self.ranges.clone();
        let mut cursor = old_ranges.cursor::<FoldOrder>(());
        let mut ranges = cursor.slice(&key, TreeBias::Left);
        let removed = cursor.item().copied().filter(|fold| fold.id() == id)?;
        cursor.next();
        ranges.append(cursor.suffix(), ());
        self.ranges = ranges;
        self.keys_by_id.remove(&id);
        self.last_patch = None;
        self.last_patch_changed_hidden_spans = false;
        if let Some(affected) = affected_hidden_span {
            self.refresh_hidden_span_after_removal(affected);
        }
        Some(removed)
    }

    /// 移除「包含给定 offset 的最内层 fold」。
    ///
    /// 优先选择 range 长度最小的命中 fold；若无 fold 命中则返回 None。
    pub fn unfold_at(&mut self, offset: ByteOffset) -> Option<FoldRange> {
        let candidate = self
            .ranges
            .iter()
            .filter(|fold| range_contains_offset(fold.range(), offset))
            .min_by_key(|fold| fold.range().len())
            .map(FoldRange::id)?;
        self.unfold(candidate)
    }

    pub fn unfold_all(&mut self) {
        self.ranges = SumTree::new(());
        self.keys_by_id.clear();
        self.hidden_spans = SumTree::new(());
        self.last_patch = None;
        self.last_patch_changed_hidden_spans = false;
    }

    /// 切换 fold 状态：若精确 range 已存在则移除并返回 `Unfolded(id)`，否则新增并返回 `Folded(id)`。
    pub fn toggle(
        &mut self,
        snapshot: &Snapshot,
        range: TextRange,
    ) -> DisplayMapResult<FoldToggleOutcome> {
        self.ensure_snapshot_version(snapshot)?;
        if range.is_empty() {
            return Err(FoldError::EmptyRange { range }.into());
        }

        if let Some(id) = self.find_exact(range) {
            self.unfold(id);
            return Ok(FoldToggleOutcome::Unfolded(id));
        }

        // 走 fold 内部路径但跳过重复的 find_exact——上面刚查过，不存在。
        self.validate_nesting(range)?;
        let line_span = fold_line_span(snapshot, range)?;
        let id = self.reserve_id()?;
        let fold = FoldRange::with_policy(
            id,
            self.version,
            range,
            self.default_update_policy,
            line_span,
        );
        self.insert_fold(fold);
        self.last_patch = None;
        self.last_patch_changed_hidden_spans = false;
        self.insert_hidden_span_for_fold(fold);
        Ok(FoldToggleOutcome::Folded(id))
    }

    /// 将 fold 集合排序到稳定形态：先按 start 升序，再按 end 降序（外层在前）。
    pub fn normalize(&mut self) {
        self.sort_ranges_and_rebuild_keys();
        self.rebuild_hidden_spans();
    }

    pub fn is_stale(&self, current_version: BufferVersion) -> bool {
        self.version != current_version
    }

    /// 查询某条逻辑行是否被任意 fold 隐藏。
    ///
    /// 一条 fold 跨越逻辑行 `[a, b]` 时，隐藏 `(a, b]`（即 `a + 1` 到 `b` 闭区间）。
    /// 单行 fold 不贡献隐藏行（占位符语义在 `projection` 模块）。
    /// 直接读 fold 缓存的 `line_span`，调用方不必再传 snapshot。
    pub fn is_line_hidden(&self, line: Line) -> bool {
        self.ranges.iter().any(|fold| {
            let (start_line, end_line) = fold.line_span();
            start_line < line && line <= end_line
        })
    }

    /// 把当前所有 fold 投影成一组排序、合并后的 `HiddenRange`。
    /// 直接读取 FoldSet 同步维护的持久化隐藏区间摘要树。
    pub fn derive_hidden_ranges(&self) -> DisplayMapResult<Vec<HiddenRange>> {
        self.hidden_spans
            .iter()
            .map(|span| LineRange::new(span.start, span.end).map(HiddenRange::new))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// 通过一个组合 Patch 把所有 fold range 推进到新版本，并返回每条 fold 的 update 事实。
    ///
    /// `snapshot` 必须为应用 Patch 后的当前快照：
    /// 用它重算每条保留下来的 fold 的 `line_span` 缓存，保持「fold 元数据与文本同版本」不变量。
    pub fn update_through_patch(
        &mut self,
        old_version: BufferVersion,
        new_version: BufferVersion,
        patch: &TextPatch,
        snapshot: &Snapshot,
    ) -> DisplayMapResult<Vec<FoldRangeUpdate>> {
        if self.version != old_version {
            return Err(FoldError::VersionMismatch {
                expected: old_version,
                actual: self.version,
            }
            .into());
        }
        if snapshot.version() != new_version {
            return Err(FoldError::VersionMismatch {
                expected: new_version,
                actual: snapshot.version(),
            }
            .into());
        }

        let position_map = PositionMap::from_text_patch(patch);
        let mut updates = Vec::with_capacity(self.len());
        let mut retained = Vec::with_capacity(self.len());
        let mut hidden_spans_changed = false;

        for mut fold in self.ranges.iter().copied() {
            let id = fold.id();
            let old_line_span = fold.line_span();
            let tracked_update = fold.tracked_range().map_through_position_map_with_policy(
                new_version,
                &position_map,
                fold.update_policy(),
            );
            let update = FoldRangeUpdate::from_tracked(id, tracked_update);

            if let Some(tracked_range) = tracked_update.tracked_range() {
                fold.set_tracked_range(tracked_range);
                let line_span = fold_line_span(snapshot, tracked_range.range())?;
                fold.set_line_span(line_span);
                hidden_spans_changed |= line_span != old_line_span;
                retained.push(fold);
            } else {
                hidden_spans_changed |= old_line_span.0 < old_line_span.1;
            }

            updates.push(update);
        }

        self.ranges = SumTree::from_iter(retained, ());
        self.version = new_version;
        self.sort_ranges_and_rebuild_keys();
        if hidden_spans_changed {
            self.rebuild_hidden_spans();
        }
        self.last_patch = Some((old_version, new_version));
        self.last_patch_changed_hidden_spans = hidden_spans_changed;
        Ok(updates)
    }

    fn ensure_snapshot_version(&self, snapshot: &Snapshot) -> DisplayMapResult<()> {
        if snapshot.version() != self.version {
            return Err(FoldError::VersionMismatch {
                expected: self.version,
                actual: snapshot.version(),
            }
            .into());
        }
        Ok(())
    }

    fn reserve_id(&mut self) -> Result<FoldRangeId, FoldError> {
        let id = self.next_id;
        self.next_id = self.next_id.next().ok_or(FoldError::IdOverflow)?;
        Ok(id)
    }

    fn find_exact(&self, range: TextRange) -> Option<FoldRangeId> {
        self.ranges
            .iter()
            .find(|fold| fold.range() == range)
            .map(FoldRange::id)
    }

    fn validate_nesting(&self, candidate: TextRange) -> Result<(), FoldError> {
        for fold in self.ranges.iter() {
            let existing = fold.range();
            if !ranges_disjoint_or_nested(existing, candidate) {
                return Err(FoldError::OverlapWithoutNesting {
                    existing,
                    candidate,
                });
            }
        }
        Ok(())
    }

    pub(crate) fn hidden_spans(&self) -> &SumTree<HiddenSpan> {
        &self.hidden_spans
    }

    pub(crate) fn was_updated_between(
        &self,
        old_version: BufferVersion,
        new_version: BufferVersion,
    ) -> bool {
        self.last_patch == Some((old_version, new_version))
    }

    pub(crate) fn hidden_spans_changed_between(
        &self,
        old_version: BufferVersion,
        new_version: BufferVersion,
    ) -> Option<bool> {
        self.was_updated_between(old_version, new_version)
            .then_some(self.last_patch_changed_hidden_spans)
    }

    fn insert_fold(&mut self, fold: FoldRange) {
        let key = FoldOrder::for_fold(&fold);
        let old_ranges = self.ranges.clone();
        let mut cursor = old_ranges.cursor::<FoldOrder>(());
        let mut ranges = cursor.slice(&key, TreeBias::Left);
        ranges.push(fold, ());
        ranges.append(cursor.suffix(), ());
        self.ranges = ranges;
        self.keys_by_id.insert(fold.id(), key);
    }

    fn rebuild_keys(&mut self) {
        self.keys_by_id = self
            .ranges
            .iter()
            .map(|fold| (fold.id(), FoldOrder::for_fold(fold)))
            .collect();
    }

    fn sort_ranges_and_rebuild_keys(&mut self) {
        let mut ranges: Vec<_> = self.ranges.iter().copied().collect();
        ranges.sort_by(|a, b| {
            let a_range = a.range();
            let b_range = b.range();
            a_range
                .start()
                .cmp(&b_range.start())
                .then_with(|| b_range.end().cmp(&a_range.end()))
                .then_with(|| a.id().cmp(&b.id()))
        });
        self.ranges = SumTree::from_iter(ranges, ());
        self.rebuild_keys();
    }

    fn hidden_span_for_fold(&self, fold: &FoldRange) -> Option<HiddenSpan> {
        let (start_line, end_line) = fold.line_span();
        (start_line < end_line).then(|| HiddenSpan {
            start: next_line(start_line),
            end: next_line(end_line),
        })
    }

    fn insert_hidden_span_for_fold(&mut self, fold: FoldRange) {
        let Some(mut candidate) = self.hidden_span_for_fold(&fold) else {
            return;
        };

        let old_spans = self.hidden_spans.clone();
        let mut cursor = old_spans.cursor::<HiddenSpanEnd>(());
        let mut spans = cursor.slice(&HiddenSpanEnd(candidate.start.get()), TreeBias::Left);
        while let Some(current) = cursor.item().copied() {
            if current.start > candidate.end {
                break;
            }
            candidate.start = candidate.start.min(current.start);
            candidate.end = candidate.end.max(current.end);
            cursor.next();
        }
        spans.push(candidate, ());
        spans.append(cursor.suffix(), ());
        self.hidden_spans = spans;
    }

    fn overlapping_hidden_span(&self, target: HiddenSpan) -> Option<HiddenSpan> {
        let (_, _, span) = self.hidden_spans.find::<HiddenSpanEnd, _>(
            (),
            &HiddenSpanEnd(target.start.get()),
            TreeBias::Left,
        );
        span.copied()
            .filter(|span| span.start <= target.end && target.start <= span.end)
    }

    fn refresh_hidden_span_after_removal(&mut self, affected: HiddenSpan) {
        let mut replacements: Vec<HiddenSpan> = Vec::new();
        for fold in self.ranges.iter() {
            let Some(span) = self.hidden_span_for_fold(fold) else {
                continue;
            };
            if span.end < affected.start || span.start > affected.end {
                continue;
            }
            match replacements.last_mut() {
                Some(last) if span.start <= last.end => {
                    last.end = last.end.max(span.end);
                }
                _ => replacements.push(span),
            }
        }

        let old_spans = self.hidden_spans.clone();
        let mut cursor = old_spans.cursor::<HiddenSpanEnd>(());
        let mut spans = cursor.slice(&HiddenSpanEnd(affected.start.get()), TreeBias::Left);
        debug_assert_eq!(cursor.item(), Some(&affected));
        cursor.next();
        spans.append(SumTree::from_iter(replacements, ()), ());
        spans.append(cursor.suffix(), ());
        self.hidden_spans = spans;
    }

    fn rebuild_hidden_spans(&mut self) {
        let mut merged: Vec<HiddenSpan> = Vec::with_capacity(self.len());
        for fold in self.ranges.iter() {
            let (start_line, end_line) = fold.line_span();
            if start_line >= end_line {
                continue;
            }
            let span = HiddenSpan {
                start: next_line(start_line),
                end: next_line(end_line),
            };
            match merged.last_mut() {
                Some(last) if span.start <= last.end => {
                    if span.end > last.end {
                        last.end = span.end;
                    }
                }
                _ => merged.push(span),
            }
        }
        self.hidden_spans = SumTree::from_iter(merged, ());
    }
}

impl Item for FoldRange {
    type Summary = FoldTreeSummary;

    fn summary(&self, (): ()) -> Self::Summary {
        FoldTreeSummary {
            count: 1,
            last_order: FoldOrder::for_fold(self),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldTreeSummary {
    count: usize,
    last_order: FoldOrder,
}

impl Default for FoldTreeSummary {
    fn default() -> Self {
        Self {
            count: 0,
            last_order: FoldOrder::zero(()),
        }
    }
}

impl ContextLessSummary for FoldTreeSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.count += summary.count;
        self.last_order = summary.last_order;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FoldOrder {
    start: usize,
    end_descending: Reverse<usize>,
    id: u64,
}

impl FoldOrder {
    fn for_fold(fold: &FoldRange) -> Self {
        Self {
            start: fold.range().start().get(),
            end_descending: Reverse(fold.range().end().get()),
            id: fold.id().get(),
        }
    }
}

impl<'a> Dimension<'a, FoldTreeSummary> for FoldOrder {
    fn zero((): ()) -> Self {
        Self {
            start: 0,
            end_descending: Reverse(usize::MAX),
            id: 0,
        }
    }

    fn add_summary(&mut self, summary: &'a FoldTreeSummary, (): ()) {
        *self = summary.last_order;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HiddenSpan {
    pub(crate) start: Line,
    pub(crate) end: Line,
}

impl Item for HiddenSpan {
    type Summary = HiddenSpanSummary;

    fn summary(&self, (): ()) -> Self::Summary {
        HiddenSpanSummary {
            count: 1,
            last_start: self.start,
            last_end: self.end,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HiddenSpanSummary {
    pub(crate) count: usize,
    pub(crate) last_start: Line,
    pub(crate) last_end: Line,
}

impl ContextLessSummary for HiddenSpanSummary {
    fn zero() -> Self {
        Self::default()
    }

    fn add_summary(&mut self, summary: &Self) {
        self.count += summary.count;
        self.last_start = summary.last_start;
        self.last_end = summary.last_end;
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct HiddenSpanEnd(pub(crate) usize);

impl<'a> Dimension<'a, HiddenSpanSummary> for HiddenSpanEnd {
    fn zero((): ()) -> Self {
        Self(0)
    }

    fn add_summary(&mut self, summary: &'a HiddenSpanSummary, (): ()) {
        self.0 = summary.last_end.get();
    }
}

fn ranges_disjoint_or_nested(left: TextRange, right: TextRange) -> bool {
    if left.end() <= right.start() || right.end() <= left.start() {
        return true;
    }

    if left.start() <= right.start() && right.end() <= left.end() {
        return true;
    }

    if right.start() <= left.start() && left.end() <= right.end() {
        return true;
    }

    false
}

fn range_contains_offset(range: TextRange, offset: ByteOffset) -> bool {
    if range.is_empty() {
        return false;
    }
    range.start() <= offset && offset < range.end()
}
