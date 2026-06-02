//! MetadataLayers 聚合：按 MetadataLayerKind 组织多组外部 metadata ranges。
//!
//! 本文件负责 layer 替换、按 kind 查询和过期丢弃，不进入单条 range 的跟随细节。

use crate::{
    buffer::Buffer,
    errors::MetadataError,
    transaction::DeltaEvent,
    types::{BufferVersion, LineRange, TextRange},
};

use super::{
    MetadataLayer, MetadataLayerKind, MetadataLineWindow, MetadataRange, MetadataRangeId,
    MetadataRangeSpec, MetadataRangeUpdate, query::text_range_for_line_range,
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

    /// 把 `event` 应用到所有 `version == event.old_version()` 的 layer，让 layer 内每条 range 沿编辑前后的字节坐标平移。
    ///
    /// **为什么是部分应用**：layer 的 version 由 producer 自主推进（worker 的 `ReplaceAll` / `ReplaceRange` 各自标版本）。
    /// 一次 `pump_post_edit` 喂事件时，刚好与 `event.old_version()` 对齐的 layer 才能推进；
    /// 其他 layer 要么已经走在前面（worker 已基于新版本算完），要么落在后面（下一轮再追赶）。
    /// 因此版本错配不算错——静默跳过，与 [`discard_stale`] 风格一致。
    ///
    /// 返回每条参与平移的 layer 的更新列表（key 为 kind），便于宿主诊断哪些 layer 实际被推进。
    pub fn update_through_delta_event(
        &mut self,
        event: &DeltaEvent,
    ) -> Vec<(MetadataLayerKind, Vec<MetadataRangeUpdate>)> {
        let mut out = Vec::new();
        for layer in self.layers.iter_mut() {
            if layer.version() != event.old_version() {
                continue;
            }
            if let Ok(updates) = layer.update_through_delta_event(event) {
                out.push((layer.kind().clone(), updates));
            }
        }
        out
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
    fn update_through_delta_event_shifts_aligned_layer_and_skips_others() {
        use crate::{Edit, Transaction};
        // 文件含多字节字符：插一字符后让旧 span 端点原本会落进 char 内部时，
        // 平移应整体把它跟过插入点。
        let mut buf = buffer("# zom 文档规范\n");
        let kind = MetadataLayerKind::custom("syntax");
        let other_kind = MetadataLayerKind::custom("other");

        let mut layers: MetadataLayers<&'static str> = MetadataLayers::new();
        // 模拟 syntax worker 在初始版本上落两段 span。
        layers
            .replace_layer_ranges(
                kind.clone(),
                buf.version(),
                vec![(range(0, 1), "punct"), (range(2, 18), "heading")],
            )
            .unwrap();
        // 同时挂一个版本「领先」的 layer，验证 update 静默跳过它。
        layers.push(MetadataLayer::with_kind(
            other_kind.clone(),
            BufferVersion::new(buf.version().get() + 10),
        ));

        // 在 byte 5（`m` 后）插入 'X'，模拟一次按键。
        let edit = Edit::insert(b(5), "X".to_string()).unwrap();
        let tx = Transaction::from_edits(buf.version(), vec![edit]).unwrap();
        buf.apply_transaction(tx).unwrap();
        let event = buf
            .pending_delta_events()
            .last()
            .expect("应至少一条 DeltaEvent")
            .clone();
        let updates = layers.update_through_delta_event(&event);
        assert_eq!(updates.len(), 1, "只有 syntax layer 版本对齐，应只推进一条");
        assert_eq!(updates[0].0, kind);

        let layer = layers.layer(&kind).unwrap();
        assert_eq!(layer.version(), event.new_version());

        let mut ranges: Vec<(usize, usize)> = layer
            .as_slice()
            .iter()
            .map(|r| (r.range().start().get(), r.range().end().get()))
            .collect();
        ranges.sort();
        // 旧 [2, 18) 的 end=18 在插入点 5 之后，整体右移 1 → [2, 19)；
        // 19 在新文本里是 newline 字节，char-aligned。
        assert_eq!(ranges, vec![(0, 1), (2, 19)]);

        // 另一 layer 版本超前，静默跳过、未被推进。
        let other = layers.layer(&other_kind).unwrap();
        assert_ne!(other.version(), event.new_version());
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
