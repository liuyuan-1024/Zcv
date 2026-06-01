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

    /// 局部替换：与 [`MetadataLayer::replace_in_range`] 同语义，但接受 `kind`
    /// 寻址。若 layer 不存在，先以 `version` 建空 layer 再走局部替换——等价于
    /// "首份 ReplaceRange 起手铺底"。版本不匹配返回 `VersionMismatch`。
    pub fn replace_layer_ranges_in_range(
        &mut self,
        kind: MetadataLayerKind,
        version: BufferVersion,
        byte_range: TextRange,
        ranges: impl IntoIterator<Item = (TextRange, T)>,
    ) -> Result<Vec<MetadataRangeId>, MetadataError> {
        if let Some(index) = self.layers.iter().position(|layer| layer.kind() == &kind) {
            return self.layers[index].replace_in_range(version, byte_range, ranges);
        }
        let mut layer = MetadataLayer::with_kind(kind, version);
        let ids = layer.replace_in_range(version, byte_range, ranges)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BufferConfig, ByteOffset, Line};

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

    #[test]
    fn metadata_layers_should_replace_query_by_kind_and_discard_stale_layers() {
        let buffer = buffer("abc\ndef");
        let mut layers = MetadataLayers::new();
        let kind = MetadataLayerKind::custom("analysis");

        layers
            .replace_layer_ranges(
                kind.clone(),
                buffer.version(),
                vec![(range(0, 3), "alpha"), (range(4, 7), "beta")],
            )
            .unwrap();

        assert_eq!(layers.len(), 1);
        assert_eq!(
            layers
                .ranges_for_kind_intersecting(&kind, range(1, 5))
                .map(|entry| *entry.metadata())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        assert_eq!(
            layers
                .ranges_for_kind_in_line_range(&kind, &buffer, line_range(1, 2))
                .unwrap()
                .len(),
            1
        );

        let stale = layers.discard_stale(BufferVersion::new(99));
        assert_eq!(stale.len(), 1);
        assert!(layers.is_empty());
    }

    #[test]
    fn replace_layer_ranges_in_range_keeps_out_of_range_spans_and_swaps_inside() {
        let buffer = buffer("abcdefghij");
        let mut layers = MetadataLayers::new();
        let kind = MetadataLayerKind::custom("syntax");

        // 起点：四段全文 spans。
        layers
            .replace_layer_ranges(
                kind.clone(),
                buffer.version(),
                vec![
                    (range(0, 2), "a"),
                    (range(2, 5), "b"),
                    (range(5, 8), "c"),
                    (range(8, 10), "d"),
                ],
            )
            .unwrap();

        // 局部替换 byte_range = [2, 8)：b 与 c 的 start 落在其中，应被替换；a 与 d 保留。
        layers
            .replace_layer_ranges_in_range(
                kind.clone(),
                buffer.version(),
                range(2, 8),
                vec![(range(2, 4), "B"), (range(4, 7), "C")],
            )
            .unwrap();

        let mut survivors: Vec<&str> = layers
            .layer(&kind)
            .unwrap()
            .as_slice()
            .iter()
            .map(|r| *r.metadata())
            .collect();
        survivors.sort();
        assert_eq!(survivors, vec!["B", "C", "a", "d"]);
    }

    #[test]
    fn replace_layer_ranges_in_range_rejects_stale_version() {
        let buffer = buffer("abcd");
        let mut layers = MetadataLayers::new();
        let kind = MetadataLayerKind::custom("syntax");
        layers
            .replace_layer_ranges(kind.clone(), buffer.version(), vec![(range(0, 4), "a")])
            .unwrap();
        let err = layers
            .replace_layer_ranges_in_range(
                kind,
                BufferVersion::new(buffer.version().get() + 1),
                range(0, 4),
                vec![(range(0, 4), "b")],
            )
            .unwrap_err();
        assert!(matches!(
            err,
            crate::errors::MetadataError::VersionMismatch { .. }
        ));
    }
}
