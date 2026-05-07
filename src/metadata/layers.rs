//! MetadataLayers 聚合：按 MetadataLayerKind 组织多组外部 metadata ranges。
//!
//! 本文件负责 layer 替换、按 kind 查询和过期丢弃，不进入单条 range 的跟随细节。

use crate::{
    buffer::Buffer,
    errors::MetadataError,
    types::{BufferVersion, LineRange, TextRange},
};

use super::{
    MetadataLayer, MetadataLayerKind, MetadataLineWindow, MetadataRange, MetadataRangeId,
    MetadataRangeSpec, query::text_range_for_line_range,
};

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

    pub fn ranges_for_kind_in_line_window(
        &self,
        kind: &MetadataLayerKind,
        buffer: &Buffer,
        window: MetadataLineWindow,
    ) -> crate::EngineResult<Vec<&MetadataRange<T>>> {
        self.ranges_for_kind_in_line_range(kind, buffer, window.lines())
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
