//! 单个 MetadataLayer：管理同一版本上的 metadata ranges、查询和版本推进。
//!
//! 本文件负责 layer 内部身份、默认跟随策略和 DeltaEvent 应用；多 layer 聚合放在 `layers.rs`。

use crate::{
    buffer::Buffer,
    errors::MetadataError,
    position_map::Stickiness,
    tracking::TrackedRangeUpdatePolicy,
    transaction::DeltaEvent,
    types::{BufferVersion, ByteOffset, LineRange, TextRange},
};

use super::{
    MetadataLayerKind, MetadataLineWindow, MetadataRange, MetadataRangeId, MetadataRangeSpec,
    MetadataRangeUpdate,
    query::{range_contains_offset, ranges_intersect, text_range_for_line_range},
};

/// 同一 BufferVersion 下的一组外部 metadata ranges。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataLayer<T> {
    kind: MetadataLayerKind,
    version: BufferVersion,
    next_id: MetadataRangeId,
    default_stickiness: Stickiness,
    default_update_policy: TrackedRangeUpdatePolicy,
    ranges: Vec<MetadataRange<T>>,
}

impl<T> MetadataLayer<T> {
    pub fn with_kind(kind: MetadataLayerKind, version: BufferVersion) -> Self {
        Self {
            kind,
            version,
            next_id: MetadataRangeId::INITIAL,
            default_stickiness: Stickiness::default(),
            default_update_policy: TrackedRangeUpdatePolicy::default(),
            ranges: Vec::new(),
        }
    }

    pub fn kind(&self) -> &MetadataLayerKind {
        &self.kind
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

    pub fn default_stickiness(&self) -> Stickiness {
        self.default_stickiness
    }

    pub fn default_update_policy(&self) -> TrackedRangeUpdatePolicy {
        self.default_update_policy
    }

    pub fn with_default_stickiness(mut self, stickiness: Stickiness) -> Self {
        self.default_stickiness = stickiness;
        self
    }

    pub fn with_default_update_policy(mut self, policy: TrackedRangeUpdatePolicy) -> Self {
        self.default_update_policy = policy;
        self
    }

    pub fn as_slice(&self) -> &[MetadataRange<T>] {
        &self.ranges
    }

    /// 拆解为 `(kind, version, default_stickiness, default_update_policy, ranges)`。
    pub fn into_parts(
        self,
    ) -> (
        MetadataLayerKind,
        BufferVersion,
        Stickiness,
        TrackedRangeUpdatePolicy,
        Vec<MetadataRange<T>>,
    ) {
        (
            self.kind,
            self.version,
            self.default_stickiness,
            self.default_update_policy,
            self.ranges,
        )
    }

    pub fn iter(&self) -> impl Iterator<Item = &MetadataRange<T>> {
        self.ranges.iter()
    }

    pub fn get(&self, id: MetadataRangeId) -> Option<&MetadataRange<T>> {
        self.ranges.iter().find(|range| range.id() == id)
    }

    pub fn get_mut(&mut self, id: MetadataRangeId) -> Option<&mut MetadataRange<T>> {
        self.ranges.iter_mut().find(|range| range.id() == id)
    }

    pub fn insert(
        &mut self,
        range: TextRange,
        metadata: T,
    ) -> Result<MetadataRangeId, MetadataError> {
        self.insert_with_options(
            range,
            self.default_stickiness,
            self.default_update_policy,
            metadata,
        )
    }

    pub fn insert_with_stickiness(
        &mut self,
        range: TextRange,
        stickiness: Stickiness,
        metadata: T,
    ) -> Result<MetadataRangeId, MetadataError> {
        self.insert_with_options(range, stickiness, self.default_update_policy, metadata)
    }

    pub fn insert_with_options(
        &mut self,
        range: TextRange,
        stickiness: Stickiness,
        update_policy: TrackedRangeUpdatePolicy,
        metadata: T,
    ) -> Result<MetadataRangeId, MetadataError> {
        let id = self.reserve_id()?;
        self.ranges.push(MetadataRange::with_policy(
            id,
            self.version,
            range,
            stickiness,
            update_policy,
            metadata,
        ));
        Ok(id)
    }

    pub fn remove(&mut self, id: MetadataRangeId) -> Option<MetadataRange<T>> {
        let index = self.ranges.iter().position(|range| range.id() == id)?;
        Some(self.ranges.remove(index))
    }

    pub fn clear(&mut self) {
        self.ranges.clear();
    }

    pub fn replace_all(
        &mut self,
        version: BufferVersion,
        ranges: impl IntoIterator<Item = (TextRange, T)>,
    ) -> Result<Vec<MetadataRangeId>, MetadataError> {
        self.replace_all_with_options(
            version,
            ranges
                .into_iter()
                .map(|(range, metadata)| MetadataRangeSpec::new(range, metadata)),
        )
    }

    pub fn replace_all_with_options(
        &mut self,
        version: BufferVersion,
        ranges: impl IntoIterator<Item = MetadataRangeSpec<T>>,
    ) -> Result<Vec<MetadataRangeId>, MetadataError> {
        let mut next_id = MetadataRangeId::INITIAL;
        let mut ids = Vec::new();
        let mut new_ranges = Vec::new();

        for spec in ranges {
            let id = next_id;
            next_id = next_id.next().ok_or(MetadataError::IdOverflow)?;
            let (range, stickiness, update_policy, metadata) = spec.into_parts();
            ids.push(id);
            new_ranges.push(MetadataRange::with_policy(
                id,
                version,
                range,
                stickiness,
                update_policy,
                metadata,
            ));
        }

        self.version = version;
        self.next_id = next_id;
        self.ranges = new_ranges;
        Ok(ids)
    }

    /// 局部替换：删除 `byte_range` 内（按 start 落点判定）的现存 ranges，再把
    /// `new_ranges` 追加进 layer。`byte_range` 外的 ranges 完全保留，版本不动。
    ///
    /// 服务于语法高亮的 viewport-scoped ReplaceRange 路径（[改造方案
    /// §3.6](../../../zom-workspace/docs/语法高亮异步增量改造.md)）：worker 只产
    /// viewport ± 缓冲区段的 spans，远处旧 spans 保持不变，避免每次编辑都全层
    /// 重建。
    ///
    /// 版本必须与 layer.version 一致；不一致返回 `VersionMismatch`，调用方应在
    /// drain 前做版本守护。`new_ranges` 不必落在 `byte_range` 内（caller 责任），
    /// 但落在该范围外的新 range 会与未删除的旧 range 共存，调用方需自行避免重叠
    /// 语义冲突。
    pub fn replace_in_range(
        &mut self,
        version: BufferVersion,
        byte_range: TextRange,
        new_ranges: impl IntoIterator<Item = (TextRange, T)>,
    ) -> Result<Vec<MetadataRangeId>, MetadataError> {
        self.replace_in_range_with_options(
            version,
            byte_range,
            new_ranges
                .into_iter()
                .map(|(range, metadata)| MetadataRangeSpec::new(range, metadata)),
        )
    }

    pub fn replace_in_range_with_options(
        &mut self,
        version: BufferVersion,
        byte_range: TextRange,
        new_ranges: impl IntoIterator<Item = MetadataRangeSpec<T>>,
    ) -> Result<Vec<MetadataRangeId>, MetadataError> {
        if self.version != version {
            return Err(MetadataError::VersionMismatch {
                expected: self.version,
                actual: version,
            });
        }
        let cutoff_start = byte_range.start();
        let cutoff_end = byte_range.end();
        self.ranges.retain(|metadata_range| {
            let start = metadata_range.range().start();
            !(start >= cutoff_start && start < cutoff_end)
        });
        let mut ids = Vec::new();
        for spec in new_ranges {
            let id = self.reserve_id()?;
            let (range, stickiness, update_policy, metadata) = spec.into_parts();
            self.ranges.push(MetadataRange::with_policy(
                id,
                version,
                range,
                stickiness,
                update_policy,
                metadata,
            ));
            ids.push(id);
        }
        Ok(ids)
    }

    pub fn is_stale(&self, current_version: BufferVersion) -> bool {
        self.version != current_version
    }

    pub fn ranges_intersecting(&self, query: TextRange) -> impl Iterator<Item = &MetadataRange<T>> {
        self.ranges
            .iter()
            .filter(move |metadata_range| ranges_intersect(metadata_range.range(), query))
    }

    pub fn ranges_containing(&self, offset: ByteOffset) -> impl Iterator<Item = &MetadataRange<T>> {
        self.ranges
            .iter()
            .filter(move |metadata_range| range_contains_offset(metadata_range.range(), offset))
    }

    pub fn ranges_in_line_range(
        &self,
        buffer: &Buffer,
        query: LineRange,
    ) -> crate::EngineResult<Vec<&MetadataRange<T>>> {
        let query = text_range_for_line_range(buffer, query)?;
        Ok(self.ranges_intersecting(query).collect())
    }

    pub fn ranges_in_line_window(
        &self,
        buffer: &Buffer,
        window: MetadataLineWindow,
    ) -> crate::EngineResult<Vec<&MetadataRange<T>>> {
        self.ranges_in_line_range(buffer, window.lines())
    }

    pub fn update_through_delta_event(
        &mut self,
        event: &DeltaEvent,
    ) -> Result<Vec<MetadataRangeUpdate>, MetadataError> {
        if self.version != event.old_version() {
            return Err(MetadataError::VersionMismatch {
                expected: event.old_version(),
                actual: self.version,
            });
        }

        let mut updates = Vec::with_capacity(self.ranges.len());
        let mut retained = Vec::with_capacity(self.ranges.len());

        for mut metadata_range in self.ranges.drain(..) {
            let id = metadata_range.id();
            let tracked_update = metadata_range
                .tracked_range()
                .map_through_position_map_with_policy(
                    event.new_version(),
                    event.position_map(),
                    metadata_range.update_policy(),
                );
            let update = MetadataRangeUpdate::from_tracked(id, tracked_update);

            if let Some(tracked_range) = tracked_update.tracked_range() {
                metadata_range.set_tracked_range(tracked_range);
                retained.push(metadata_range);
            }

            updates.push(update);
        }

        self.ranges = retained;
        self.version = event.new_version();
        Ok(updates)
    }

    fn reserve_id(&mut self) -> Result<MetadataRangeId, MetadataError> {
        let id = self.next_id;
        self.next_id = self.next_id.next().ok_or(MetadataError::IdOverflow)?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BufferConfig, Edit, Line, LineRange, Transaction};

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
    fn metadata_layer_should_insert_query_update_and_drop_invalidated_ranges() {
        let mut buffer = buffer("one\ntwo\nthree");
        let mut layer =
            MetadataLayer::with_kind(MetadataLayerKind::custom("notes"), buffer.version())
                .with_default_update_policy(
                    TrackedRangeUpdatePolicy::invalidate_when_fully_deleted(),
                );
        let first = layer.insert(range(0, 3), "one").unwrap();
        let second = layer.insert(range(4, 7), "two").unwrap();

        assert_eq!(layer.len(), 2);
        assert_eq!(layer.get(first).unwrap().metadata(), &"one");
        assert_eq!(
            layer
                .ranges_intersecting(range(2, 5))
                .map(|entry| *entry.metadata())
                .collect::<Vec<_>>(),
            vec!["one", "two"]
        );
        assert_eq!(
            layer
                .ranges_in_line_range(&buffer, line_range(1, 2))
                .unwrap()
                .len(),
            1
        );

        let event = event_after(&mut buffer, Edit::delete(range(4, 7)));
        let updates = layer.update_through_delta_event(&event).unwrap();

        assert_eq!(updates.len(), 2);
        assert_eq!(layer.version(), event.new_version());
        assert!(layer.get(second).is_none());
        assert_eq!(layer.len(), 1);
    }
}
