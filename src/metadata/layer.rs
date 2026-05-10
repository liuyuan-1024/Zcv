//! 单个 MetadataLayer：管理同一版本上的 metadata ranges、查询和版本推进。
//!
//! 本文件负责 layer 内部身份、默认跟随策略和 DeltaEvent 应用；多 layer 聚合放在 `layers.rs`。

use crate::{
    buffer::Buffer,
    errors::MetadataError,
    position_map::Stickiness,
    tracking::TrackedRangeUpdatePolicy,
    transaction::DeltaEvent,
    types::{BufferVersion, CharOffset, LineRange, TextRange},
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
    pub fn new(version: BufferVersion) -> Self {
        Self::with_kind(MetadataLayerKind::default(), version)
    }

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

    pub fn is_stale(&self, current_version: BufferVersion) -> bool {
        self.version != current_version
    }

    pub fn ranges_intersecting(&self, query: TextRange) -> impl Iterator<Item = &MetadataRange<T>> {
        self.ranges
            .iter()
            .filter(move |metadata_range| ranges_intersect(metadata_range.range(), query))
    }

    pub fn ranges_containing(&self, offset: CharOffset) -> impl Iterator<Item = &MetadataRange<T>> {
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
        if self.version != event.old_version {
            return Err(MetadataError::VersionMismatch {
                expected: event.old_version,
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
                    event.new_version,
                    &event.position_map,
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
        self.version = event.new_version;
        Ok(updates)
    }

    fn reserve_id(&mut self) -> Result<MetadataRangeId, MetadataError> {
        let id = self.next_id;
        self.next_id = self.next_id.next().ok_or(MetadataError::IdOverflow)?;
        Ok(id)
    }
}
