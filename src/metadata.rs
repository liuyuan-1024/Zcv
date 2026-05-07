//! M10A MetadataLayer：把外部 metadata 绑定到可跟随文本变化的区间上。
//!
//! 本模块不定义 diagnostics、高亮、断点等业务 payload，只负责承载泛型
//! metadata、绑定 BufferVersion、复用 TrackedRange 的范围追踪语义，并提供基础查询。

use crate::{
    errors::{AnchorError, MetadataError},
    position_map::Stickiness,
    tracked_range::{TrackedRange, TrackedRangeUpdate, TrackedRangeUpdatePolicy},
    transaction::DeltaEvent,
    types::{BufferVersion, CharOffset, TextRange},
};

/// MetadataRange 在单个 MetadataLayer 内的稳定身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct MetadataRangeId(u64);

impl MetadataRangeId {
    pub const INITIAL: Self = Self(0);

    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

/// MetadataLayer 的通用类别。
///
/// 这些类别只用于分层和查询，不代表引擎会生成对应业务含义。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MetadataLayerKind {
    SearchMatch,
    Diagnostics,
    SyntaxHighlight,
    SemanticToken,
    Breakpoint,
    Bookmark,
    InlayHint,
    CodeLens,
    Custom(String),
}

impl MetadataLayerKind {
    pub fn custom(name: impl Into<String>) -> Self {
        Self::Custom(name.into())
    }
}

impl Default for MetadataLayerKind {
    fn default() -> Self {
        Self::Custom(String::new())
    }
}

/// 单条外部 metadata 与其可跟随文本区间。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataRange<T> {
    id: MetadataRangeId,
    tracked_range: TrackedRange,
    update_policy: TrackedRangeUpdatePolicy,
    metadata: T,
}

impl<T> MetadataRange<T> {
    pub fn new(
        id: MetadataRangeId,
        version: BufferVersion,
        range: TextRange,
        stickiness: Stickiness,
        metadata: T,
    ) -> Self {
        Self::with_policy(
            id,
            version,
            range,
            stickiness,
            TrackedRangeUpdatePolicy::default(),
            metadata,
        )
    }

    pub fn with_policy(
        id: MetadataRangeId,
        version: BufferVersion,
        range: TextRange,
        stickiness: Stickiness,
        update_policy: TrackedRangeUpdatePolicy,
        metadata: T,
    ) -> Self {
        Self {
            id,
            tracked_range: TrackedRange::from_range(version, range, stickiness),
            update_policy,
            metadata,
        }
    }

    pub fn id(&self) -> MetadataRangeId {
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

    pub fn metadata(&self) -> &T {
        &self.metadata
    }

    pub fn metadata_mut(&mut self) -> &mut T {
        &mut self.metadata
    }

    pub fn into_metadata(self) -> T {
        self.metadata
    }

    pub fn map_through_delta_event(
        &self,
        event: &DeltaEvent,
    ) -> Result<TrackedRangeUpdate, AnchorError> {
        self.tracked_range
            .map_through_delta_event_with_policy(event, self.update_policy)
    }

    fn set_tracked_range(&mut self, tracked_range: TrackedRange) {
        self.tracked_range = tracked_range;
    }
}

/// 单条 metadata range 通过一次 DeltaEvent 后的更新事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetadataRangeUpdate {
    Mapped {
        id: MetadataRangeId,
        range: TextRange,
        version: BufferVersion,
    },
    Deleted {
        id: MetadataRangeId,
        range: TextRange,
        version: BufferVersion,
    },
    Collapsed {
        id: MetadataRangeId,
        range: TextRange,
        version: BufferVersion,
    },
    Invalidated {
        id: MetadataRangeId,
        range: TextRange,
        version: BufferVersion,
    },
}

impl MetadataRangeUpdate {
    pub fn id(self) -> MetadataRangeId {
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

    fn from_tracked(id: MetadataRangeId, update: TrackedRangeUpdate) -> Self {
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

fn ranges_intersect(left: TextRange, right: TextRange) -> bool {
    match (left.is_empty(), right.is_empty()) {
        (true, true) => left.start() == right.start(),
        (true, false) => right.start() <= left.start() && left.start() < right.end(),
        (false, true) => left.start() <= right.start() && right.start() < left.end(),
        (false, false) => left.start() < right.end() && right.start() < left.end(),
    }
}

fn range_contains_offset(range: TextRange, offset: CharOffset) -> bool {
    if range.is_empty() {
        return range.start() == offset;
    }

    range.start() <= offset && offset < range.end()
}
