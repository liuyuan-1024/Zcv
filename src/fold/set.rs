//! FoldSet：同一 BufferVersion 下的折叠区间集合。
//!
//! FoldSet 维护折叠集合自身的不变量：
//! - id 单 FoldSet 内单调递增；
//! - 任意两个 fold 之间必须满足「互不相交」或「严格嵌套」，禁止部分重叠；
//! - 当通过 `DeltaEvent` 推进版本时，每条 fold 按其 `TrackedRangeUpdatePolicy` 决定保留 / 塌缩 / 失效。
//!
//! 折叠占位符样式、投影坐标和 viewport 切片属于 M13B 起，不在本文件承诺。

use crate::{
    EngineResult,
    buffer::Buffer,
    errors::FoldError,
    tracking::TrackedRangeUpdatePolicy,
    transaction::DeltaEvent,
    types::{BufferVersion, CharOffset, Line, LineRange, TextRange},
};

use super::{
    FoldRange, FoldRangeId, FoldRangeUpdate, HiddenRange,
    geometry::{char_range_for_line_range, fold_line_span, next_line},
    range::default_update_policy,
};

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
    ranges: Vec<FoldRange>,
}

impl FoldSet {
    pub fn new(version: BufferVersion) -> Self {
        Self {
            version,
            next_id: FoldRangeId::INITIAL,
            default_update_policy: default_update_policy(),
            ranges: Vec::new(),
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
        self.ranges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn default_update_policy(&self) -> TrackedRangeUpdatePolicy {
        self.default_update_policy
    }

    pub fn as_slice(&self) -> &[FoldRange] {
        &self.ranges
    }

    pub fn iter(&self) -> impl Iterator<Item = &FoldRange> {
        self.ranges.iter()
    }

    pub fn get(&self, id: FoldRangeId) -> Option<&FoldRange> {
        self.ranges.iter().find(|fold| fold.id() == id)
    }

    /// 折叠任意合法 char range；若已存在精确相同的 range 则返回该 fold 的 id（幂等）。
    pub fn fold(&mut self, range: TextRange) -> Result<FoldRangeId, FoldError> {
        self.fold_with_policy(range, self.default_update_policy)
    }

    pub fn fold_with_policy(
        &mut self,
        range: TextRange,
        update_policy: TrackedRangeUpdatePolicy,
    ) -> Result<FoldRangeId, FoldError> {
        if range.is_empty() {
            return Err(FoldError::EmptyRange { range });
        }

        if let Some(existing) = self.find_exact(range) {
            return Ok(existing);
        }

        self.validate_nesting(range)?;
        let id = self.reserve_id()?;
        self.ranges.push(FoldRange::with_policy(
            id,
            self.version,
            range,
            update_policy,
        ));
        self.normalize();
        Ok(id)
    }

    /// 按 line range 折叠：line range 转换为对应 char range 后走通用 fold 入口。
    pub fn fold_lines(
        &mut self,
        buffer: &Buffer,
        line_range: LineRange,
    ) -> EngineResult<FoldRangeId> {
        let range = char_range_for_line_range(buffer, line_range)?;
        Ok(self.fold(range)?)
    }

    pub fn fold_lines_with_policy(
        &mut self,
        buffer: &Buffer,
        line_range: LineRange,
        update_policy: TrackedRangeUpdatePolicy,
    ) -> EngineResult<FoldRangeId> {
        let range = char_range_for_line_range(buffer, line_range)?;
        Ok(self.fold_with_policy(range, update_policy)?)
    }

    /// 移除指定 id 的 fold。返回被移除的 FoldRange；若 id 不存在则返回 None。
    pub fn unfold(&mut self, id: FoldRangeId) -> Option<FoldRange> {
        let index = self.ranges.iter().position(|fold| fold.id() == id)?;
        Some(self.ranges.remove(index))
    }

    /// 移除「包含给定 offset 的最内层 fold」。
    ///
    /// 优先选择 range 长度最小的命中 fold；若无 fold 命中则返回 None。
    pub fn unfold_at(&mut self, offset: CharOffset) -> Option<FoldRange> {
        let candidate = self
            .ranges
            .iter()
            .enumerate()
            .filter(|(_, fold)| range_contains_offset(fold.range(), offset))
            .min_by_key(|(_, fold)| fold.range().len())
            .map(|(idx, _)| idx)?;
        Some(self.ranges.remove(candidate))
    }

    pub fn unfold_all(&mut self) {
        self.ranges.clear();
    }

    /// 切换 fold 状态：若精确 range 已存在则移除并返回 `Unfolded(id)`，否则新增并返回 `Folded(id)`。
    pub fn toggle(&mut self, range: TextRange) -> Result<FoldToggleOutcome, FoldError> {
        if range.is_empty() {
            return Err(FoldError::EmptyRange { range });
        }

        if let Some(id) = self.find_exact(range) {
            self.unfold(id);
            return Ok(FoldToggleOutcome::Unfolded(id));
        }

        let id = self.fold(range)?;
        Ok(FoldToggleOutcome::Folded(id))
    }

    /// 将 fold 集合排序到稳定形态：先按 start 升序，再按 end 降序（外层在前）。
    pub fn normalize(&mut self) {
        self.ranges.sort_by(|a, b| {
            let a_range = a.range();
            let b_range = b.range();
            a_range
                .start()
                .cmp(&b_range.start())
                .then_with(|| b_range.end().cmp(&a_range.end()))
                .then_with(|| a.id().cmp(&b.id()))
        });
    }

    pub fn is_stale(&self, current_version: BufferVersion) -> bool {
        self.version != current_version
    }

    /// 查询某条逻辑行是否被任意 fold 隐藏。
    ///
    /// 一条 fold 跨越逻辑行 `[a, b]` 时，隐藏 `(a, b]`（即 `a + 1` 到 `b` 闭区间）。
    /// 单行 fold 不贡献隐藏行（占位符语义留给 M13B）。
    pub fn is_line_hidden(&self, buffer: &Buffer, line: Line) -> EngineResult<bool> {
        for fold in &self.ranges {
            let (start_line, end_line) = fold_line_span(buffer, fold.range())?;
            if start_line < line && line <= end_line {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// 把当前所有 fold 投影成一组排序、合并后的 `HiddenRange`。
    pub fn derive_hidden_ranges(&self, buffer: &Buffer) -> EngineResult<Vec<HiddenRange>> {
        let mut spans: Vec<(Line, Line)> = Vec::new();
        for fold in &self.ranges {
            let (start_line, end_line) = fold_line_span(buffer, fold.range())?;
            if start_line < end_line {
                spans.push((next_line(start_line), next_line(end_line)));
            }
        }

        spans.sort_by_key(|&(start, _)| start);
        let mut merged: Vec<(Line, Line)> = Vec::with_capacity(spans.len());
        for (start, end) in spans {
            match merged.last_mut() {
                Some(last) if start <= last.1 => {
                    if end > last.1 {
                        last.1 = end;
                    }
                }
                _ => merged.push((start, end)),
            }
        }

        merged
            .into_iter()
            .map(|(start, end)| LineRange::new(start, end).map(HiddenRange::new))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    /// 通过一次 DeltaEvent 把所有 fold range 推进到新版本，并返回每条 fold 的 update 事实。
    pub fn update_through_delta_event(
        &mut self,
        event: &DeltaEvent,
    ) -> Result<Vec<FoldRangeUpdate>, FoldError> {
        if self.version != event.old_version {
            return Err(FoldError::VersionMismatch {
                expected: event.old_version,
                actual: self.version,
            });
        }

        let mut updates = Vec::with_capacity(self.ranges.len());
        let mut retained = Vec::with_capacity(self.ranges.len());

        for mut fold in self.ranges.drain(..) {
            let id = fold.id();
            let tracked_update = fold.tracked_range().map_through_position_map_with_policy(
                event.new_version,
                &event.position_map,
                fold.update_policy(),
            );
            let update = FoldRangeUpdate::from_tracked(id, tracked_update);

            if let Some(tracked_range) = tracked_update.tracked_range() {
                fold.set_tracked_range(tracked_range);
                retained.push(fold);
            }

            updates.push(update);
        }

        self.ranges = retained;
        self.version = event.new_version;
        self.normalize();
        Ok(updates)
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
        for fold in &self.ranges {
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

fn range_contains_offset(range: TextRange, offset: CharOffset) -> bool {
    if range.is_empty() {
        return false;
    }
    range.start() <= offset && offset < range.end()
}
