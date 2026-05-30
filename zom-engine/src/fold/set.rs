//! FoldSet：同一 BufferVersion 下的折叠区间集合。
//!
//! FoldSet 维护折叠集合自身的不变量：
//! - id 单 FoldSet 内单调递增；
//! - 任意两个 fold 之间必须满足「互不相交」或「严格嵌套」，禁止部分重叠；
//! - 当通过 `DeltaEvent` 推进版本时，每条 fold 按其 `TrackedRangeUpdatePolicy` 决定保留 / 塌缩 / 失效。
//!
//! 折叠占位符样式、投影坐标和 viewport 切片由 `projection` 模块承载，不在本文件承诺。

use crate::{
    EngineResult,
    errors::FoldError,
    snapshot::Snapshot,
    tracking::TrackedRangeUpdatePolicy,
    transaction::DeltaEvent,
    types::{BufferVersion, ByteOffset, Line, LineRange, TextRange},
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

    /// 折叠任意合法 byte range；若已存在精确相同的 range 则返回该 fold 的 id（幂等）。
    ///
    /// `snapshot` 必须与本 FoldSet 同版本：构造时 eager 缓存 fold 的逻辑行跨度，
    /// 让后续 `Projection::build` / 增量分类器无需再为每条 fold 做 byte→line 转换。
    pub fn fold(&mut self, snapshot: &Snapshot, range: TextRange) -> EngineResult<FoldRangeId> {
        self.fold_with_policy(snapshot, range, self.default_update_policy)
    }

    pub fn fold_with_policy(
        &mut self,
        snapshot: &Snapshot,
        range: TextRange,
        update_policy: TrackedRangeUpdatePolicy,
    ) -> EngineResult<FoldRangeId> {
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
        self.ranges.push(FoldRange::with_policy(
            id,
            self.version,
            range,
            update_policy,
            line_span,
        ));
        self.normalize();
        Ok(id)
    }

    /// 按 line range 折叠：line range 转换为对应 byte range 后走通用 fold 入口。
    pub fn fold_lines(
        &mut self,
        snapshot: &Snapshot,
        line_range: LineRange,
    ) -> EngineResult<FoldRangeId> {
        self.ensure_snapshot_version(snapshot)?;
        let range = char_range_for_line_range(snapshot, line_range)?;
        self.fold(snapshot, range)
    }

    pub fn fold_lines_with_policy(
        &mut self,
        snapshot: &Snapshot,
        line_range: LineRange,
        update_policy: TrackedRangeUpdatePolicy,
    ) -> EngineResult<FoldRangeId> {
        self.ensure_snapshot_version(snapshot)?;
        let range = char_range_for_line_range(snapshot, line_range)?;
        self.fold_with_policy(snapshot, range, update_policy)
    }

    /// 移除指定 id 的 fold。返回被移除的 FoldRange；若 id 不存在则返回 None。
    pub fn unfold(&mut self, id: FoldRangeId) -> Option<FoldRange> {
        let index = self.ranges.iter().position(|fold| fold.id() == id)?;
        Some(self.ranges.remove(index))
    }

    /// 移除「包含给定 offset 的最内层 fold」。
    ///
    /// 优先选择 range 长度最小的命中 fold；若无 fold 命中则返回 None。
    pub fn unfold_at(&mut self, offset: ByteOffset) -> Option<FoldRange> {
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
    pub fn toggle(
        &mut self,
        snapshot: &Snapshot,
        range: TextRange,
    ) -> EngineResult<FoldToggleOutcome> {
        self.ensure_snapshot_version(snapshot)?;
        if range.is_empty() {
            return Err(FoldError::EmptyRange { range }.into());
        }

        if let Some(id) = self.find_exact(range) {
            self.unfold(id);
            return Ok(FoldToggleOutcome::Unfolded(id));
        }

        let id = self.fold(snapshot, range)?;
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
    /// 单行 fold 不贡献隐藏行（占位符语义在 `projection` 模块）。
    /// 直接读 fold 缓存的 `line_span`，调用方不必再传 snapshot。
    pub fn is_line_hidden(&self, line: Line) -> bool {
        self.ranges.iter().any(|fold| {
            let (start_line, end_line) = fold.line_span();
            start_line < line && line <= end_line
        })
    }

    /// 把当前所有 fold 投影成一组排序、合并后的 `HiddenRange`。
    /// 读 fold 缓存的 `line_span`，与 `Projection::build` 的 `collect_merged_hidden_spans`
    /// 共享同一份语义。
    pub fn derive_hidden_ranges(&self) -> EngineResult<Vec<HiddenRange>> {
        let mut merged: Vec<(Line, Line)> = Vec::with_capacity(self.ranges.len());
        // `normalize` 已让 `ranges` 按 byte-start 升序，
        // 字节升序 → 缓存的 start_line 非降序；可以边收集边合并。
        for fold in &self.ranges {
            let (start_line, end_line) = fold.line_span();
            if start_line >= end_line {
                continue;
            }
            let start = next_line(start_line);
            let end = next_line(end_line);
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
    ///
    /// `snapshot` 必须为应用 delta 后的快照（版本等于 `event.new_version()`）：
    /// 用它重算每条保留下来的 fold 的 `line_span` 缓存，保持「fold 元数据与文本同版本」不变量。
    pub fn update_through_delta_event(
        &mut self,
        event: &DeltaEvent,
        snapshot: &Snapshot,
    ) -> EngineResult<Vec<FoldRangeUpdate>> {
        if self.version != event.old_version() {
            return Err(FoldError::VersionMismatch {
                expected: event.old_version(),
                actual: self.version,
            }
            .into());
        }
        if snapshot.version() != event.new_version() {
            return Err(FoldError::VersionMismatch {
                expected: event.new_version(),
                actual: snapshot.version(),
            }
            .into());
        }

        let mut updates = Vec::with_capacity(self.ranges.len());
        let mut retained = Vec::with_capacity(self.ranges.len());

        for mut fold in self.ranges.drain(..) {
            let id = fold.id();
            let tracked_update = fold.tracked_range().map_through_position_map_with_policy(
                event.new_version(),
                event.position_map(),
                fold.update_policy(),
            );
            let update = FoldRangeUpdate::from_tracked(id, tracked_update);

            if let Some(tracked_range) = tracked_update.tracked_range() {
                fold.set_tracked_range(tracked_range);
                let line_span = fold_line_span(snapshot, tracked_range.range())?;
                fold.set_line_span(line_span);
                retained.push(fold);
            }

            updates.push(update);
        }

        self.ranges = retained;
        self.version = event.new_version();
        self.normalize();
        Ok(updates)
    }

    fn ensure_snapshot_version(&self, snapshot: &Snapshot) -> EngineResult<()> {
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

fn range_contains_offset(range: TextRange, offset: ByteOffset) -> bool {
    if range.is_empty() {
        return false;
    }
    range.start() <= offset && offset < range.end()
}
