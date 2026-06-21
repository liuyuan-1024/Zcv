//! `VersionedRangeSet<T>`：把 (TrackedRange, payload) 与 BufferVersion 绑定的核心集合。
//!
//! 每条 entry 由 set 分配稳定 `VersionedRangeEntryId`，跨 delta 不变；
//! 调用方通过 id 访问、删除或读取 entry，而不是位置下标——位置下标在 `update_through_delta_event` 失效条目后会发生漂移。
//!
//! 自定义分析产物等容器直接持有。

use crate::{
    EngineResult,
    buffer::Buffer,
    errors::VersionedResultError,
    position_map::Stickiness,
    snapshot::Snapshot,
    tracking::{TrackedRange, TrackedRangeUpdate, TrackedRangeUpdatePolicy},
    transaction::DeltaEvent,
    types::{BufferVersion, ByteOffset, LineRange, TextRange, Utf16Position},
};

use super::query::{range_contains_offset, ranges_intersect, text_range_for_line_range};

/// VersionedRangeEntry 在单个 set 内的稳定身份。
///
/// 仅在 set 生命周期内稳定，不跨 set 表达全局身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VersionedRangeEntryId(u64);

impl VersionedRangeEntryId {
    const INITIAL: Self = Self(0);

    pub const fn get(self) -> u64 {
        self.0
    }

    fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

/// 单条 entry：把 payload 绑定到一个可跟随的 `TrackedRange`，附带稳定 id。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionedRangeEntry<T> {
    id: VersionedRangeEntryId,
    tracked_range: TrackedRange,
    update_policy: TrackedRangeUpdatePolicy,
    payload: T,
}

impl<T> VersionedRangeEntry<T> {
    fn new(
        id: VersionedRangeEntryId,
        tracked_range: TrackedRange,
        update_policy: TrackedRangeUpdatePolicy,
        payload: T,
    ) -> Self {
        Self {
            id,
            tracked_range,
            update_policy,
            payload,
        }
    }

    pub fn id(&self) -> VersionedRangeEntryId {
        self.id
    }

    pub fn version(&self) -> BufferVersion {
        self.tracked_range.version()
    }

    pub fn range(&self) -> TextRange {
        self.tracked_range.range()
    }

    pub fn tracked_range(&self) -> TrackedRange {
        self.tracked_range
    }

    pub fn stickiness(&self) -> Stickiness {
        self.tracked_range.stickiness()
    }

    pub fn update_policy(&self) -> TrackedRangeUpdatePolicy {
        self.update_policy
    }

    pub fn payload(&self) -> &T {
        &self.payload
    }

    pub fn payload_mut(&mut self) -> &mut T {
        &mut self.payload
    }

    pub fn into_payload(self) -> T {
        self.payload
    }

    pub fn into_parts(
        self,
    ) -> (
        VersionedRangeEntryId,
        TrackedRange,
        TrackedRangeUpdatePolicy,
        T,
    ) {
        (
            self.id,
            self.tracked_range,
            self.update_policy,
            self.payload,
        )
    }

    pub fn map_through_delta_event(
        &self,
        event: &DeltaEvent,
    ) -> Result<TrackedRangeUpdate, crate::errors::AnchorError> {
        self.tracked_range
            .map_through_delta_event_with_policy(event, self.update_policy)
    }

    fn set_tracked_range(&mut self, tracked_range: TrackedRange) {
        self.tracked_range = tracked_range;
    }
}

/// 批量替换或构造 entry 时使用的输入项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionedRangeSpec<T> {
    range: TextRange,
    stickiness: Stickiness,
    update_policy: TrackedRangeUpdatePolicy,
    payload: T,
}

impl<T> VersionedRangeSpec<T> {
    pub fn new(range: TextRange, payload: T) -> Self {
        Self {
            range,
            stickiness: Stickiness::default(),
            update_policy: TrackedRangeUpdatePolicy::default(),
            payload,
        }
    }

    pub fn with_stickiness(mut self, stickiness: Stickiness) -> Self {
        self.stickiness = stickiness;
        self
    }

    pub fn with_update_policy(mut self, update_policy: TrackedRangeUpdatePolicy) -> Self {
        self.update_policy = update_policy;
        self
    }

    pub fn range(&self) -> TextRange {
        self.range
    }

    pub fn stickiness(&self) -> Stickiness {
        self.stickiness
    }

    pub fn update_policy(&self) -> TrackedRangeUpdatePolicy {
        self.update_policy
    }

    pub fn payload(&self) -> &T {
        &self.payload
    }

    /// 用 UTF-16 行列边界构造 spec，便于从外部协议（LSP 等）批量导入。
    ///
    /// `start` / `end` 必须落在 `snapshot` 的合法 UTF-16 边界内，且 `start <= end`。
    pub fn try_from_utf16(
        snapshot: &Snapshot,
        start: Utf16Position,
        end: Utf16Position,
        payload: T,
    ) -> EngineResult<Self> {
        let range = utf16_range_to_text_range(snapshot, start, end)?;
        Ok(Self::new(range, payload))
    }

    fn into_parts(self) -> (TextRange, Stickiness, TrackedRangeUpdatePolicy, T) {
        (
            self.range,
            self.stickiness,
            self.update_policy,
            self.payload,
        )
    }
}

/// 单条 entry 通过一次 `DeltaEvent` 后的更新事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VersionedRangeUpdate {
    /// 区间无删除触碰地推进到新版本，entry 可以原样保留。
    Mapped {
        id: VersionedRangeEntryId,
        range: TextRange,
        version: BufferVersion,
    },
    /// 原区间被删除内容触碰，但按策略仍保留折算后的区间。
    Deleted {
        id: VersionedRangeEntryId,
        range: TextRange,
        version: BufferVersion,
    },
    /// 原非空区间映射后成为空区间，并且当前策略允许继续保留。
    Collapsed {
        id: VersionedRangeEntryId,
        range: TextRange,
        version: BufferVersion,
    },
    /// 当前 update policy 判定 entry 不应继续存在，返回最后合法位置供宿主清理或展示。
    Invalidated {
        id: VersionedRangeEntryId,
        range: TextRange,
        version: BufferVersion,
    },
}

impl VersionedRangeUpdate {
    pub fn id(self) -> VersionedRangeEntryId {
        match self {
            Self::Mapped { id, .. }
            | Self::Deleted { id, .. }
            | Self::Collapsed { id, .. }
            | Self::Invalidated { id, .. } => id,
        }
    }

    pub fn range(self) -> TextRange {
        match self {
            Self::Mapped { range, .. }
            | Self::Deleted { range, .. }
            | Self::Collapsed { range, .. }
            | Self::Invalidated { range, .. } => range,
        }
    }

    pub fn version(self) -> BufferVersion {
        match self {
            Self::Mapped { version, .. }
            | Self::Deleted { version, .. }
            | Self::Collapsed { version, .. }
            | Self::Invalidated { version, .. } => version,
        }
    }

    pub fn is_invalidated(self) -> bool {
        matches!(self, Self::Invalidated { .. })
    }

    fn from_tracked(id: VersionedRangeEntryId, update: TrackedRangeUpdate) -> Self {
        match update {
            TrackedRangeUpdate::Mapped(range) => Self::Mapped {
                id,
                range: range.range(),
                version: range.version(),
            },
            TrackedRangeUpdate::Deleted(range) => Self::Deleted {
                id,
                range: range.range(),
                version: range.version(),
            },
            TrackedRangeUpdate::Collapsed(range) => Self::Collapsed {
                id,
                range: range.range(),
                version: range.version(),
            },
            TrackedRangeUpdate::Invalidated { range, version } => {
                Self::Invalidated { id, range, version }
            }
        }
    }
}

/// 版本化的 (id, TrackedRange, payload) 集合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionedRangeSet<T> {
    version: BufferVersion,
    next_id: VersionedRangeEntryId,
    default_stickiness: Stickiness,
    default_update_policy: TrackedRangeUpdatePolicy,
    entries: Vec<VersionedRangeEntry<T>>,
}

impl<T> VersionedRangeSet<T> {
    pub fn new(version: BufferVersion) -> Self {
        Self {
            version,
            next_id: VersionedRangeEntryId::INITIAL,
            default_stickiness: Stickiness::default(),
            default_update_policy: TrackedRangeUpdatePolicy::default(),
            entries: Vec::new(),
        }
    }

    pub fn with_default_stickiness(mut self, stickiness: Stickiness) -> Self {
        self.default_stickiness = stickiness;
        self
    }

    pub fn with_default_update_policy(mut self, policy: TrackedRangeUpdatePolicy) -> Self {
        self.default_update_policy = policy;
        self
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    pub fn is_stale(&self, current: BufferVersion) -> bool {
        self.version != current
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn default_stickiness(&self) -> Stickiness {
        self.default_stickiness
    }

    pub fn default_update_policy(&self) -> TrackedRangeUpdatePolicy {
        self.default_update_policy
    }

    pub fn as_slice(&self) -> &[VersionedRangeEntry<T>] {
        &self.entries
    }

    pub fn iter(&self) -> impl Iterator<Item = &VersionedRangeEntry<T>> {
        self.entries.iter()
    }

    /// 按稳定 id 查询 entry。
    pub fn get(&self, id: VersionedRangeEntryId) -> Option<&VersionedRangeEntry<T>> {
        self.entries.iter().find(|entry| entry.id() == id)
    }

    pub fn get_mut(&mut self, id: VersionedRangeEntryId) -> Option<&mut VersionedRangeEntry<T>> {
        self.entries.iter_mut().find(|entry| entry.id() == id)
    }

    /// 追加 entry，返回新分配的稳定 id。
    pub fn insert(
        &mut self,
        range: TextRange,
        payload: T,
    ) -> Result<VersionedRangeEntryId, VersionedResultError> {
        self.insert_with_options(
            range,
            self.default_stickiness,
            self.default_update_policy,
            payload,
        )
    }

    pub fn insert_with_stickiness(
        &mut self,
        range: TextRange,
        stickiness: Stickiness,
        payload: T,
    ) -> Result<VersionedRangeEntryId, VersionedResultError> {
        self.insert_with_options(range, stickiness, self.default_update_policy, payload)
    }

    pub fn insert_with_options(
        &mut self,
        range: TextRange,
        stickiness: Stickiness,
        update_policy: TrackedRangeUpdatePolicy,
        payload: T,
    ) -> Result<VersionedRangeEntryId, VersionedResultError> {
        let id = self.reserve_id()?;
        let tracked_range = TrackedRange::from_range(self.version, range, stickiness);
        self.entries.push(VersionedRangeEntry::new(
            id,
            tracked_range,
            update_policy,
            payload,
        ));
        Ok(id)
    }

    /// 删除指定 id 的 entry。
    pub fn remove(&mut self, id: VersionedRangeEntryId) -> Option<VersionedRangeEntry<T>> {
        let index = self.entries.iter().position(|entry| entry.id() == id)?;
        Some(self.entries.remove(index))
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn replace_all(
        &mut self,
        version: BufferVersion,
        ranges: impl IntoIterator<Item = (TextRange, T)>,
    ) -> Result<Vec<VersionedRangeEntryId>, VersionedResultError> {
        self.replace_all_with_options(
            version,
            ranges
                .into_iter()
                .map(|(range, payload)| VersionedRangeSpec::new(range, payload)),
        )
    }

    pub fn replace_all_with_options(
        &mut self,
        version: BufferVersion,
        specs: impl IntoIterator<Item = VersionedRangeSpec<T>>,
    ) -> Result<Vec<VersionedRangeEntryId>, VersionedResultError> {
        let mut next_id = VersionedRangeEntryId::INITIAL;
        let mut ids = Vec::new();
        let mut new_entries = Vec::new();

        for spec in specs {
            let id = next_id;
            next_id = next_id.next().ok_or(VersionedResultError::IdOverflow)?;
            let (range, stickiness, update_policy, payload) = spec.into_parts();
            let tracked_range = TrackedRange::from_range(version, range, stickiness);
            ids.push(id);
            new_entries.push(VersionedRangeEntry::new(
                id,
                tracked_range,
                update_policy,
                payload,
            ));
        }

        self.version = version;
        self.next_id = next_id;
        self.entries = new_entries;
        Ok(ids)
    }

    /// 局部替换：删除 `byte_range` 内（按 start 落点判定）的现存 entry，再把 `new_ranges` 追加进 set。
    /// `byte_range` 外的 entry 完全保留，版本不动。
    ///
    /// 版本必须与 set.version 一致；不一致返回 `VersionMismatch`。
    /// `new_ranges` 不必落在 `byte_range` 内（caller 责任），但落在该范围外的新 entry 会与未删除的旧 entry 共存，调用方需自行避免重叠语义冲突。
    pub fn replace_in_range(
        &mut self,
        version: BufferVersion,
        byte_range: TextRange,
        new_ranges: impl IntoIterator<Item = (TextRange, T)>,
    ) -> Result<Vec<VersionedRangeEntryId>, VersionedResultError> {
        self.replace_in_range_with_options(
            version,
            byte_range,
            new_ranges
                .into_iter()
                .map(|(range, payload)| VersionedRangeSpec::new(range, payload)),
        )
    }

    pub fn replace_in_range_with_options(
        &mut self,
        version: BufferVersion,
        byte_range: TextRange,
        new_ranges: impl IntoIterator<Item = VersionedRangeSpec<T>>,
    ) -> Result<Vec<VersionedRangeEntryId>, VersionedResultError> {
        if self.version != version {
            return Err(VersionedResultError::VersionMismatch {
                expected: self.version,
                actual: version,
            });
        }
        let cutoff_start = byte_range.start();
        let cutoff_end = byte_range.end();
        self.entries.retain(|entry| {
            let start = entry.range().start();
            !(start >= cutoff_start && start < cutoff_end)
        });
        let mut ids = Vec::new();
        for spec in new_ranges {
            let id = self.reserve_id()?;
            let (range, stickiness, update_policy, payload) = spec.into_parts();
            let tracked_range = TrackedRange::from_range(version, range, stickiness);
            self.entries.push(VersionedRangeEntry::new(
                id,
                tracked_range,
                update_policy,
                payload,
            ));
            ids.push(id);
        }
        Ok(ids)
    }

    /// 应用一次 `DeltaEvent`：按每条 entry 的 update policy 推进 tracked range；
    /// 失效的 entry 在 set 内删除，返回所有 entry（含失效条目）的更新事实。
    pub fn update_through_delta_event(
        &mut self,
        event: &DeltaEvent,
    ) -> Result<Vec<VersionedRangeUpdate>, VersionedResultError> {
        if self.version != event.old_version() {
            return Err(VersionedResultError::VersionMismatch {
                expected: event.old_version(),
                actual: self.version,
            });
        }

        let mut updates = Vec::with_capacity(self.entries.len());
        let mut retained = Vec::with_capacity(self.entries.len());

        for mut entry in self.entries.drain(..) {
            let id = entry.id();
            let tracked_update = entry.tracked_range.map_through_position_map_with_policy(
                event.new_version(),
                event.position_map(),
                entry.update_policy,
            );
            let update = VersionedRangeUpdate::from_tracked(id, tracked_update);

            if let Some(tracked_range) = tracked_update.tracked_range() {
                entry.set_tracked_range(tracked_range);
                retained.push(entry);
            }

            updates.push(update);
        }

        self.entries = retained;
        self.version = event.new_version();
        Ok(updates)
    }

    pub fn entries_intersecting(
        &self,
        query: TextRange,
    ) -> impl Iterator<Item = &VersionedRangeEntry<T>> {
        self.entries
            .iter()
            .filter(move |entry| ranges_intersect(entry.range(), query))
    }

    pub fn entries_containing(
        &self,
        offset: ByteOffset,
    ) -> impl Iterator<Item = &VersionedRangeEntry<T>> {
        self.entries
            .iter()
            .filter(move |entry| range_contains_offset(entry.range(), offset))
    }

    pub fn entries_in_line_range(
        &self,
        buffer: &Buffer,
        query: LineRange,
    ) -> EngineResult<Vec<&VersionedRangeEntry<T>>> {
        let query = text_range_for_line_range(buffer, query)?;
        Ok(self.entries_intersecting(query).collect())
    }

    /// 在与 set 同版本的 `Snapshot` 上转换每条 entry 的 payload，常用于 UTF-16 边界 import / export。
    ///
    /// `snapshot.version()` 必须等于当前 `version()`；闭包接收 (payload, current_text_range, snapshot)，
    /// 不允许移动或拆解 entry 的 tracked range / stickiness / update policy。entry id 沿用。
    pub fn try_map_payloads_at_snapshot<U, F>(
        self,
        snapshot: &Snapshot,
        mut f: F,
    ) -> EngineResult<VersionedRangeSet<U>>
    where
        F: FnMut(T, TextRange, &Snapshot) -> EngineResult<U>,
    {
        if snapshot.version() != self.version {
            return Err(VersionedResultError::VersionMismatch {
                expected: self.version,
                actual: snapshot.version(),
            }
            .into());
        }

        let mut new_entries = Vec::with_capacity(self.entries.len());
        for entry in self.entries.into_iter() {
            let (id, tracked_range, update_policy, payload) = entry.into_parts();
            let range = tracked_range.range();
            let new_payload = f(payload, range, snapshot)?;
            new_entries.push(VersionedRangeEntry::new(
                id,
                tracked_range,
                update_policy,
                new_payload,
            ));
        }

        Ok(VersionedRangeSet {
            version: self.version,
            next_id: self.next_id,
            default_stickiness: self.default_stickiness,
            default_update_policy: self.default_update_policy,
            entries: new_entries,
        })
    }

    /// 把每条 entry 的范围导出为 UTF-16 行列对，便于喂给外部协议（LSP 等）。
    ///
    /// 返回 `(id, start, end, &payload)` 顺序与 `as_slice()` 一致。
    pub fn try_export_entries_to_utf16<'a>(
        &'a self,
        snapshot: &Snapshot,
    ) -> EngineResult<Vec<(VersionedRangeEntryId, Utf16Position, Utf16Position, &'a T)>> {
        if snapshot.version() != self.version {
            return Err(VersionedResultError::VersionMismatch {
                expected: self.version,
                actual: snapshot.version(),
            }
            .into());
        }

        let mut exported = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            let range = entry.range();
            let start = snapshot.byte_to_utf16_position(range.start())?;
            let end = snapshot.byte_to_utf16_position(range.end())?;
            exported.push((entry.id(), start, end, entry.payload()));
        }
        Ok(exported)
    }

    /// 用 UTF-16 行列边界追加单条 entry，便于从外部协议（LSP 等）逐条导入。
    pub fn try_insert_utf16_range(
        &mut self,
        snapshot: &Snapshot,
        start: Utf16Position,
        end: Utf16Position,
        payload: T,
    ) -> EngineResult<VersionedRangeEntryId> {
        self.try_insert_utf16_range_with_options(
            snapshot,
            start,
            end,
            self.default_stickiness,
            self.default_update_policy,
            payload,
        )
    }

    /// 用 UTF-16 行列边界追加单条 entry 并显式指定 stickiness / update policy。
    pub fn try_insert_utf16_range_with_options(
        &mut self,
        snapshot: &Snapshot,
        start: Utf16Position,
        end: Utf16Position,
        stickiness: Stickiness,
        update_policy: TrackedRangeUpdatePolicy,
        payload: T,
    ) -> EngineResult<VersionedRangeEntryId> {
        if snapshot.version() != self.version {
            return Err(VersionedResultError::VersionMismatch {
                expected: self.version,
                actual: snapshot.version(),
            }
            .into());
        }

        let range = utf16_range_to_text_range(snapshot, start, end)?;
        Ok(self.insert_with_options(range, stickiness, update_policy, payload)?)
    }

    fn reserve_id(&mut self) -> Result<VersionedRangeEntryId, VersionedResultError> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .next()
            .ok_or(VersionedResultError::IdOverflow)?;
        Ok(id)
    }
}

fn utf16_range_to_text_range(
    snapshot: &Snapshot,
    start: Utf16Position,
    end: Utf16Position,
) -> EngineResult<TextRange> {
    let start_offset = snapshot.utf16_position_to_byte(start)?;
    let end_offset = snapshot.utf16_position_to_byte(end)?;
    Ok(TextRange::new(start_offset, end_offset)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BufferConfig, Edit, Line, LineRange, Transaction, Utf16Offset};

    fn b(value: usize) -> ByteOffset {
        ByteOffset::new(value)
    }

    fn line(value: usize) -> Line {
        Line::new(value)
    }

    fn range(start: usize, end: usize) -> TextRange {
        TextRange::new(b(start), b(end)).unwrap()
    }

    fn line_range(start: usize, end: usize) -> LineRange {
        LineRange::new(line(start), line(end)).unwrap()
    }

    fn buffer(text: &str) -> Buffer {
        Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
    }

    fn event_after(buffer: &mut Buffer, edit: Edit) -> DeltaEvent {
        buffer
            .apply_transaction(Transaction::from_edits(buffer.version(), vec![edit]).unwrap())
            .unwrap();
        buffer.last_delta_event().unwrap().clone()
    }

    #[test]
    fn versioned_range_set_should_insert_query_update_and_keep_ids_stable() {
        let mut buffer = buffer("a😀b\ncd");
        let snapshot = buffer.snapshot();
        let mut set = VersionedRangeSet::new(buffer.version())
            .with_default_update_policy(TrackedRangeUpdatePolicy::invalidate_when_fully_deleted());

        let emoji_id = set
            .try_insert_utf16_range(
                &snapshot,
                Utf16Position::new(line(0), Utf16Offset::new(1)),
                Utf16Position::new(line(0), Utf16Offset::new(3)),
                "emoji",
            )
            .unwrap();
        let cd_id = set.insert(range(7, 9), "cd").unwrap();

        assert_ne!(emoji_id, cd_id);
        assert_eq!(set.entries_containing(b(1)).count(), 1);
        assert_eq!(
            set.entries_in_line_range(&buffer, line_range(1, 2))
                .unwrap()
                .len(),
            1
        );
        let exported = set.try_export_entries_to_utf16(&snapshot).unwrap();
        assert_eq!(exported[0].0, emoji_id);
        assert_eq!(exported[0].3, &"emoji");

        let event = event_after(&mut buffer, Edit::insert(b(0), "Z".to_string()).unwrap());
        let updates = set.update_through_delta_event(&event).unwrap();
        assert_eq!(updates.len(), 2);
        assert_eq!(set.version(), event.new_version());
        // id 跨 delta 不变：仍可按原 id 查询到平移后的 entry。
        assert_eq!(set.get(emoji_id).unwrap().range(), range(2, 6));
        assert_eq!(set.get(cd_id).unwrap().range(), range(8, 10));

        // 删一条后剩下的 entry 仍可按原 id 命中（位置下标会漂移，id 不会）。
        set.remove(emoji_id).unwrap();
        assert!(set.get(emoji_id).is_none());
        assert_eq!(set.get(cd_id).unwrap().payload(), &"cd");
    }

    #[test]
    fn replace_in_range_drops_inside_and_keeps_outside() {
        let buffer = buffer("abcdefghij");
        let mut set = VersionedRangeSet::<&str>::new(buffer.version());
        set.replace_all(
            buffer.version(),
            vec![
                (range(0, 2), "a"),
                (range(2, 5), "b"),
                (range(5, 8), "c"),
                (range(8, 10), "d"),
            ],
        )
        .unwrap();

        set.replace_in_range(
            buffer.version(),
            range(2, 8),
            vec![(range(2, 4), "B"), (range(4, 7), "C")],
        )
        .unwrap();

        let mut survivors: Vec<&str> = set
            .as_slice()
            .iter()
            .map(|entry| *entry.payload())
            .collect();
        survivors.sort();
        assert_eq!(survivors, vec!["B", "C", "a", "d"]);
    }
}
