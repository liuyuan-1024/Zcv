//! M10 MetadataLayer：把外部 metadata 绑定到可跟随文本变化的区间上。
//!
//! 本模块不定义 diagnostics、高亮、断点等业务 payload，只负责承载泛型
//! metadata、绑定 BufferVersion、复用 TrackedRange 的范围追踪语义，并提供基础查询。

use crate::{
    buffer::Buffer,
    errors::{AnchorError, CoordinateError, MetadataError},
    position_map::Stickiness,
    tracked_range::{TrackedRange, TrackedRangeUpdate, TrackedRangeUpdatePolicy},
    transaction::DeltaEvent,
    types::{BufferVersion, CharOffset, Line, LineRange, TextRange},
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

/// Metadata viewport 查询窗口。
///
/// M10B 只表达可见逻辑行范围，不涉及 UI 渲染、像素滚动或折叠投影坐标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MetadataViewport {
    visible_lines: LineRange,
}

impl MetadataViewport {
    pub fn new(visible_lines: LineRange) -> Self {
        Self { visible_lines }
    }

    pub fn from_lines(start: Line, end: Line) -> Result<Self, CoordinateError> {
        Ok(Self::new(LineRange::new(start, end)?))
    }

    pub fn visible_lines(self) -> LineRange {
        self.visible_lines
    }
}

/// 批量替换 metadata layer 时使用的输入项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataRangeSpec<T> {
    range: TextRange,
    stickiness: Stickiness,
    update_policy: TrackedRangeUpdatePolicy,
    metadata: T,
}

impl<T> MetadataRangeSpec<T> {
    pub fn new(range: TextRange, metadata: T) -> Self {
        Self {
            range,
            stickiness: Stickiness::default(),
            update_policy: TrackedRangeUpdatePolicy::default(),
            metadata,
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

    pub fn metadata(&self) -> &T {
        &self.metadata
    }

    fn into_parts(self) -> (TextRange, Stickiness, TrackedRangeUpdatePolicy, T) {
        (
            self.range,
            self.stickiness,
            self.update_policy,
            self.metadata,
        )
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

    pub fn ranges_in_viewport(
        &self,
        buffer: &Buffer,
        viewport: MetadataViewport,
    ) -> crate::EngineResult<Vec<&MetadataRange<T>>> {
        self.ranges_in_line_range(buffer, viewport.visible_lines())
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

/// 多个 metadata layers 的轻量集合，供宿主按 layer kind 查询、替换和丢弃过期结果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MetadataLayers<T> {
    layers: Vec<MetadataLayer<T>>,
}

impl<T> MetadataLayers<T> {
    pub fn new() -> Self {
        Self { layers: Vec::new() }
    }

    pub fn from_layers(layers: impl IntoIterator<Item = MetadataLayer<T>>) -> Self {
        Self {
            layers: layers.into_iter().collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.layers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    pub fn as_slice(&self) -> &[MetadataLayer<T>] {
        &self.layers
    }

    pub fn iter(&self) -> impl Iterator<Item = &MetadataLayer<T>> {
        self.layers.iter()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut MetadataLayer<T>> {
        self.layers.iter_mut()
    }

    pub fn push(&mut self, layer: MetadataLayer<T>) {
        self.layers.push(layer);
    }

    pub fn layer(&self, kind: &MetadataLayerKind) -> Option<&MetadataLayer<T>> {
        self.layers.iter().find(|layer| layer.kind() == kind)
    }

    pub fn layer_mut(&mut self, kind: &MetadataLayerKind) -> Option<&mut MetadataLayer<T>> {
        self.layers.iter_mut().find(|layer| layer.kind() == kind)
    }

    pub fn layers_of_kind(
        &self,
        kind: &MetadataLayerKind,
    ) -> impl Iterator<Item = &MetadataLayer<T>> {
        self.layers.iter().filter(move |layer| layer.kind() == kind)
    }

    pub fn replace_layer(&mut self, layer: MetadataLayer<T>) -> Option<MetadataLayer<T>> {
        if let Some(index) = self
            .layers
            .iter()
            .position(|existing| existing.kind() == layer.kind())
        {
            return Some(std::mem::replace(&mut self.layers[index], layer));
        }

        self.layers.push(layer);
        None
    }

    pub fn replace_layer_ranges(
        &mut self,
        kind: MetadataLayerKind,
        version: BufferVersion,
        ranges: impl IntoIterator<Item = (TextRange, T)>,
    ) -> Result<Vec<MetadataRangeId>, MetadataError> {
        self.replace_layer_ranges_with_options(
            kind,
            version,
            ranges
                .into_iter()
                .map(|(range, metadata)| MetadataRangeSpec::new(range, metadata)),
        )
    }

    pub fn replace_layer_ranges_with_options(
        &mut self,
        kind: MetadataLayerKind,
        version: BufferVersion,
        ranges: impl IntoIterator<Item = MetadataRangeSpec<T>>,
    ) -> Result<Vec<MetadataRangeId>, MetadataError> {
        if let Some(index) = self.layers.iter().position(|layer| layer.kind() == &kind) {
            return self.layers[index].replace_all_with_options(version, ranges);
        }

        let mut layer = MetadataLayer::with_kind(kind, version);
        let ids = layer.replace_all_with_options(version, ranges)?;
        self.layers.push(layer);
        Ok(ids)
    }

    pub fn ranges_for_kind_intersecting(
        &self,
        kind: &MetadataLayerKind,
        query: TextRange,
    ) -> impl Iterator<Item = &MetadataRange<T>> {
        self.layers_of_kind(kind)
            .flat_map(move |layer| layer.ranges_intersecting(query))
    }

    pub fn ranges_for_kind_in_line_range(
        &self,
        kind: &MetadataLayerKind,
        buffer: &Buffer,
        query: LineRange,
    ) -> crate::EngineResult<Vec<&MetadataRange<T>>> {
        let query = text_range_for_line_range(buffer, query)?;
        Ok(self.ranges_for_kind_intersecting(kind, query).collect())
    }

    pub fn ranges_for_kind_in_viewport(
        &self,
        kind: &MetadataLayerKind,
        buffer: &Buffer,
        viewport: MetadataViewport,
    ) -> crate::EngineResult<Vec<&MetadataRange<T>>> {
        self.ranges_for_kind_in_line_range(kind, buffer, viewport.visible_lines())
    }

    pub fn discard_stale(&mut self, current_version: BufferVersion) -> Vec<MetadataLayer<T>> {
        let mut stale = Vec::new();
        let mut index = 0;

        while index < self.layers.len() {
            if self.layers[index].is_stale(current_version) {
                stale.push(self.layers.remove(index));
            } else {
                index += 1;
            }
        }

        stale
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

fn text_range_for_line_range(
    buffer: &Buffer,
    line_range: LineRange,
) -> crate::EngineResult<TextRange> {
    let start = char_offset_for_line_boundary(buffer, line_range.start())?;
    let end = char_offset_for_line_boundary(buffer, line_range.end())?;
    Ok(TextRange::new(start, end)?)
}

fn char_offset_for_line_boundary(buffer: &Buffer, line: Line) -> crate::EngineResult<CharOffset> {
    let line_value = line.get();
    let line_count = buffer.line_count();

    if line_value > line_count {
        return Err(CoordinateError::LineOutOfBounds(line).into());
    }

    if line_value == line_count {
        return Ok(buffer.len_chars());
    }

    buffer.line_start(line)
}
