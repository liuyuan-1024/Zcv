//! `VersionedRangeSet<T>`：泛型 payload + `TrackedRange` 的版本化集合。
//!
//! 与 `MetadataLayer<T>` 的差异：不携带 `MetadataLayerKind`，不为 entry 分配稳定 ID；
//! 主要面向不需要 layer 业务身份的宿主分析产物（解析树节点、外部 range 标注等）。

use crate::{
    EngineResult,
    buffer::Buffer,
    errors::VersionedResultError,
    metadata::{
        MetadataLayer, MetadataLayerKind, MetadataLineWindow, MetadataRangeSpec,
        query::{range_contains_offset, ranges_intersect, text_range_for_line_range},
    },
    position_map::Stickiness,
    snapshot::Snapshot,
    tracking::{TrackedRange, TrackedRangeUpdate, TrackedRangeUpdatePolicy},
    transaction::DeltaEvent,
    types::{BufferVersion, ByteOffset, LineRange, TextRange, Utf16Position},
};

/// 单条 entry：把 payload 绑定到一个可跟随的 `TrackedRange`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionedRangeEntry<T> {
    tracked_range: TrackedRange,
    update_policy: TrackedRangeUpdatePolicy,
    payload: T,
}

impl<T> VersionedRangeEntry<T> {
    fn new(
        tracked_range: TrackedRange,
        update_policy: TrackedRangeUpdatePolicy,
        payload: T,
    ) -> Self {
        Self {
            tracked_range,
            update_policy,
            payload,
        }
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

    pub fn into_parts(self) -> (TrackedRange, TrackedRangeUpdatePolicy, T) {
        (self.tracked_range, self.update_policy, self.payload)
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

/// 版本化的 (TrackedRange, payload) 集合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionedRangeSet<T> {
    version: BufferVersion,
    default_stickiness: Stickiness,
    default_update_policy: TrackedRangeUpdatePolicy,
    entries: Vec<VersionedRangeEntry<T>>,
}

impl<T> VersionedRangeSet<T> {
    pub fn new(version: BufferVersion) -> Self {
        Self {
            version,
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

    pub fn entry(&self, index: usize) -> Option<&VersionedRangeEntry<T>> {
        self.entries.get(index)
    }

    pub fn entry_mut(&mut self, index: usize) -> Option<&mut VersionedRangeEntry<T>> {
        self.entries.get_mut(index)
    }

    /// 追加 entry，返回新 entry 在 set 内的索引。
    pub fn insert(&mut self, range: TextRange, payload: T) -> usize {
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
    ) -> usize {
        self.insert_with_options(range, stickiness, self.default_update_policy, payload)
    }

    pub fn insert_with_options(
        &mut self,
        range: TextRange,
        stickiness: Stickiness,
        update_policy: TrackedRangeUpdatePolicy,
        payload: T,
    ) -> usize {
        let tracked_range = TrackedRange::from_range(self.version, range, stickiness);
        let index = self.entries.len();
        self.entries.push(VersionedRangeEntry::new(
            tracked_range,
            update_policy,
            payload,
        ));
        index
    }

    pub fn remove(&mut self, index: usize) -> Option<VersionedRangeEntry<T>> {
        if index >= self.entries.len() {
            return None;
        }
        Some(self.entries.remove(index))
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn replace_all(
        &mut self,
        version: BufferVersion,
        ranges: impl IntoIterator<Item = (TextRange, T)>,
    ) {
        self.replace_all_with_options(
            version,
            ranges
                .into_iter()
                .map(|(range, payload)| VersionedRangeSpec::new(range, payload)),
        );
    }

    pub fn replace_all_with_options(
        &mut self,
        version: BufferVersion,
        specs: impl IntoIterator<Item = VersionedRangeSpec<T>>,
    ) {
        let entries = specs
            .into_iter()
            .map(|spec| {
                let (range, stickiness, update_policy, payload) = spec.into_parts();
                let tracked_range = TrackedRange::from_range(version, range, stickiness);
                VersionedRangeEntry::new(tracked_range, update_policy, payload)
            })
            .collect();

        self.version = version;
        self.entries = entries;
    }

    /// 应用一次 `DeltaEvent`，按每个 entry 的 update policy 推进 tracked range。
    ///
    /// 返回与 entry 原顺序对齐的 `TrackedRangeUpdate` 列表（含 `Invalidated`）；
    /// 失效的 entry 在 set 内删除，未失效的保留并更新 tracked range。
    pub fn update_through_delta_event(
        &mut self,
        event: &DeltaEvent,
    ) -> Result<Vec<TrackedRangeUpdate>, VersionedResultError> {
        if self.version != event.old_version() {
            return Err(VersionedResultError::VersionMismatch {
                expected: event.old_version(),
                actual: self.version,
            });
        }

        let mut updates = Vec::with_capacity(self.entries.len());
        let mut retained = Vec::with_capacity(self.entries.len());

        for mut entry in self.entries.drain(..) {
            let tracked_update = entry.tracked_range.map_through_position_map_with_policy(
                event.new_version(),
                event.position_map(),
                entry.update_policy,
            );

            if let Some(tracked_range) = tracked_update.tracked_range() {
                entry.set_tracked_range(tracked_range);
                retained.push(entry);
            }

            updates.push(tracked_update);
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

    pub fn entries_in_line_window(
        &self,
        buffer: &Buffer,
        window: MetadataLineWindow,
    ) -> EngineResult<Vec<&VersionedRangeEntry<T>>> {
        self.entries_in_line_range(buffer, window.lines())
    }

    /// 在与 set 同版本的 `Snapshot` 上转换每条 entry 的 payload，常用于 UTF-16 边界 import / export。
    ///
    /// `snapshot.version()` 必须等于当前 `version()`；闭包接收 (payload, current_text_range, snapshot)，
    /// 不允许移动或拆解 entry 的 tracked range / stickiness / update policy。
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
            let (tracked_range, update_policy, payload) = entry.into_parts();
            let range = tracked_range.range();
            let new_payload = f(payload, range, snapshot)?;
            new_entries.push(VersionedRangeEntry::new(
                tracked_range,
                update_policy,
                new_payload,
            ));
        }

        Ok(VersionedRangeSet {
            version: self.version,
            default_stickiness: self.default_stickiness,
            default_update_policy: self.default_update_policy,
            entries: new_entries,
        })
    }

    /// 把每条 entry 的范围导出为 UTF-16 行列对，便于喂给外部协议（LSP 等）。
    ///
    /// 返回 `(start, end, &payload)` 顺序与 `as_slice()` 一致。
    pub fn try_export_entries_to_utf16<'a>(
        &'a self,
        snapshot: &Snapshot,
    ) -> EngineResult<Vec<(Utf16Position, Utf16Position, &'a T)>> {
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
            // 边界投影：byte -> utf16
            let start = snapshot.byte_to_utf16_position(range.start())?;
            let end = snapshot.byte_to_utf16_position(range.end())?;
            exported.push((start, end, entry.payload()));
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
    ) -> EngineResult<usize> {
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
    ) -> EngineResult<usize> {
        if snapshot.version() != self.version {
            return Err(VersionedResultError::VersionMismatch {
                expected: self.version,
                actual: snapshot.version(),
            }
            .into());
        }

        let range = utf16_range_to_text_range(snapshot, start, end)?;
        Ok(self.insert_with_options(range, stickiness, update_policy, payload))
    }

    /// 把 set 转换为指定 kind 的 `MetadataLayer<T>`，沿用版本、默认策略和每条 entry 的策略。
    pub fn into_metadata_layer(self, kind: MetadataLayerKind) -> MetadataLayer<T> {
        let version = self.version;
        let default_stickiness = self.default_stickiness;
        let default_update_policy = self.default_update_policy;
        let mut layer = MetadataLayer::with_kind(kind, version)
            .with_default_stickiness(default_stickiness)
            .with_default_update_policy(default_update_policy);

        let specs = self.entries.into_iter().map(|entry| {
            let (tracked_range, update_policy, payload) = entry.into_parts();
            MetadataRangeSpec::new(tracked_range.range(), payload)
                .with_stickiness(tracked_range.stickiness())
                .with_update_policy(update_policy)
        });

        layer
            .replace_all_with_options(version, specs)
            .expect("entry 数量受 Vec 容量限制，远低于 MetadataRangeId u64 上限");
        layer
    }
}

impl<T> From<MetadataLayer<T>> for VersionedRangeSet<T> {
    fn from(layer: MetadataLayer<T>) -> Self {
        let (_kind, version, default_stickiness, default_update_policy, ranges) =
            layer.into_parts();
        let entries = ranges
            .into_iter()
            .map(|range| {
                let (_id, tracked_range, update_policy, payload) = range.into_parts();
                VersionedRangeEntry::new(tracked_range, update_policy, payload)
            })
            .collect();

        Self {
            version,
            default_stickiness,
            default_update_policy,
            entries,
        }
    }
}

fn utf16_range_to_text_range(
    snapshot: &Snapshot,
    start: Utf16Position,
    end: Utf16Position,
) -> EngineResult<TextRange> {
    // 边界投影：utf16 -> byte（深核区间类型）
    let start_offset = snapshot.utf16_position_to_byte(start)?;
    let end_offset = snapshot.utf16_position_to_byte(end)?;
    Ok(TextRange::new(start_offset, end_offset)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BufferConfig, Edit, Line, LineRange, Transaction, Utf16Offset, metadata::MetadataLayerKind,
    };

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
    fn versioned_range_set_should_update_query_export_utf16_and_roundtrip_metadata_layer() {
        let mut buffer = buffer("a😀b\ncd");
        let snapshot = buffer.snapshot();
        let mut set = VersionedRangeSet::new(buffer.version());

        let idx = set
            .try_insert_utf16_range(
                &snapshot,
                Utf16Position::new(line(0), Utf16Offset::new(1)),
                Utf16Position::new(line(0), Utf16Offset::new(3)),
                "emoji",
            )
            .unwrap();
        set.insert(range(7, 9), "cd");

        assert_eq!(idx, 0);
        assert_eq!(set.entries_containing(b(1)).count(), 1);
        assert_eq!(
            set.entries_in_line_range(&buffer, line_range(1, 2))
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            set.try_export_entries_to_utf16(&snapshot).unwrap()[0],
            (
                Utf16Position::new(line(0), Utf16Offset::new(1)),
                Utf16Position::new(line(0), Utf16Offset::new(3)),
                &"emoji"
            )
        );

        let event = event_after(&mut buffer, Edit::insert(b(0), "Z".to_string()).unwrap());
        let updates = set.update_through_delta_event(&event).unwrap();
        assert_eq!(updates.len(), 2);
        assert_eq!(set.version(), event.new_version());
        assert_eq!(set.entry(0).unwrap().range(), range(2, 6));

        let layer = set.into_metadata_layer(MetadataLayerKind::custom("roundtrip"));
        let roundtrip: VersionedRangeSet<&str> = VersionedRangeSet::from(layer);
        assert_eq!(roundtrip.len(), 2);
        assert_eq!(roundtrip.entry(0).unwrap().payload(), &"emoji");
    }
}
