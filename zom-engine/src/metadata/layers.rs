//! MetadataLayers：按 `MetadataLayerKind` 索引若干 `VersionedRangeSet<T>`。
//!
//! kind 只作为查询 / 替换的寻址键，不进入单个 set 的内禀状态。多 layer 的查询与替换全部委托给底层 `VersionedRangeSet`。

use crate::{
    EngineResult,
    buffer::Buffer,
    errors::VersionedResultError,
    transaction::DeltaEvent,
    types::{BufferVersion, LineRange, TextRange},
    versioned::{
        VersionedRangeEntry, VersionedRangeEntryId, VersionedRangeSet, VersionedRangeSpec,
        VersionedRangeUpdate,
    },
};

use super::MetadataLayerKind;

/// 多个按 kind 索引的 `VersionedRangeSet`，供宿主按业务分类查询、替换和丢弃过期结果。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MetadataLayers<T> {
    layers: Vec<(MetadataLayerKind, VersionedRangeSet<T>)>,
}

impl<T> MetadataLayers<T> {
    pub fn new() -> Self {
        Self { layers: Vec::new() }
    }

    pub fn from_layers(
        layers: impl IntoIterator<Item = (MetadataLayerKind, VersionedRangeSet<T>)>,
    ) -> Self {
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

    pub fn as_slice(&self) -> &[(MetadataLayerKind, VersionedRangeSet<T>)] {
        &self.layers
    }

    pub fn iter(&self) -> impl Iterator<Item = &(MetadataLayerKind, VersionedRangeSet<T>)> {
        self.layers.iter()
    }

    pub fn iter_mut(
        &mut self,
    ) -> impl Iterator<Item = &mut (MetadataLayerKind, VersionedRangeSet<T>)> {
        self.layers.iter_mut()
    }

    /// 插入或替换：若已存在同 kind 的 layer 则覆盖并返回旧 set，否则追加并返回 `None`。
    pub fn insert(
        &mut self,
        kind: MetadataLayerKind,
        set: VersionedRangeSet<T>,
    ) -> Option<VersionedRangeSet<T>> {
        if let Some(index) = self.layers.iter().position(|(k, _)| k == &kind) {
            let (_, existing) = std::mem::replace(&mut self.layers[index], (kind, set));
            return Some(existing);
        }

        self.layers.push((kind, set));
        None
    }

    pub fn layer(&self, kind: &MetadataLayerKind) -> Option<&VersionedRangeSet<T>> {
        self.layers
            .iter()
            .find(|(k, _)| k == kind)
            .map(|(_, set)| set)
    }

    pub fn layer_mut(&mut self, kind: &MetadataLayerKind) -> Option<&mut VersionedRangeSet<T>> {
        self.layers
            .iter_mut()
            .find(|(k, _)| k == kind)
            .map(|(_, set)| set)
    }

    pub fn layers_of_kind(
        &self,
        kind: &MetadataLayerKind,
    ) -> impl Iterator<Item = &VersionedRangeSet<T>> {
        self.layers
            .iter()
            .filter(move |(k, _)| k == kind)
            .map(|(_, set)| set)
    }

    /// 按 kind 整盘替换：layer 不存在则在 `version` 上新建。
    pub fn replace_layer_ranges(
        &mut self,
        kind: MetadataLayerKind,
        version: BufferVersion,
        ranges: impl IntoIterator<Item = (TextRange, T)>,
    ) -> Result<Vec<VersionedRangeEntryId>, VersionedResultError> {
        self.replace_layer_ranges_with_options(
            kind,
            version,
            ranges
                .into_iter()
                .map(|(range, payload)| VersionedRangeSpec::new(range, payload)),
        )
    }

    pub fn replace_layer_ranges_with_options(
        &mut self,
        kind: MetadataLayerKind,
        version: BufferVersion,
        ranges: impl IntoIterator<Item = VersionedRangeSpec<T>>,
    ) -> Result<Vec<VersionedRangeEntryId>, VersionedResultError> {
        if let Some(set) = self.layer_mut(&kind) {
            return set.replace_all_with_options(version, ranges);
        }

        let mut set = VersionedRangeSet::new(version);
        let ids = set.replace_all_with_options(version, ranges)?;
        self.layers.push((kind, set));
        Ok(ids)
    }

    /// 按 kind 局部替换：与 [`VersionedRangeSet::replace_in_range`] 同语义。
    /// 若 layer 不存在，先以 `version` 建空 set 再走局部替换。版本不匹配返回 `VersionMismatch`。
    pub fn replace_layer_ranges_in_range(
        &mut self,
        kind: MetadataLayerKind,
        version: BufferVersion,
        byte_range: TextRange,
        ranges: impl IntoIterator<Item = (TextRange, T)>,
    ) -> Result<Vec<VersionedRangeEntryId>, VersionedResultError> {
        if let Some(set) = self.layer_mut(&kind) {
            return set.replace_in_range(version, byte_range, ranges);
        }
        let mut set = VersionedRangeSet::new(version);
        let ids = set.replace_in_range(version, byte_range, ranges)?;
        self.layers.push((kind, set));
        Ok(ids)
    }

    pub fn ranges_for_kind_intersecting(
        &self,
        kind: &MetadataLayerKind,
        query: TextRange,
    ) -> impl Iterator<Item = &VersionedRangeEntry<T>> {
        self.layers_of_kind(kind)
            .flat_map(move |set| set.entries_intersecting(query))
    }

    pub fn ranges_for_kind_in_line_range(
        &self,
        kind: &MetadataLayerKind,
        buffer: &Buffer,
        query: LineRange,
    ) -> EngineResult<Vec<&VersionedRangeEntry<T>>> {
        let query = crate::versioned::query::text_range_for_line_range(buffer, query)?;
        Ok(self.ranges_for_kind_intersecting(kind, query).collect())
    }

    /// 把 `event` 应用到所有 `version == event.old_version()` 的 layer，让 layer 内每条 entry 沿编辑前后的字节坐标平移。
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
    ) -> Vec<(MetadataLayerKind, Vec<VersionedRangeUpdate>)> {
        let mut out = Vec::new();
        for (kind, set) in self.layers.iter_mut() {
            if set.version() != event.old_version() {
                continue;
            }
            if let Ok(updates) = set.update_through_delta_event(event) {
                out.push((kind.clone(), updates));
            }
        }
        out
    }

    pub fn discard_stale(
        &mut self,
        current_version: BufferVersion,
    ) -> Vec<(MetadataLayerKind, VersionedRangeSet<T>)> {
        let mut stale = Vec::new();
        let mut index = 0;

        while index < self.layers.len() {
            if self.layers[index].1.is_stale(current_version) {
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
                .map(|entry| *entry.payload())
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
            .map(|entry| *entry.payload())
            .collect();
        survivors.sort();
        assert_eq!(survivors, vec!["B", "C", "a", "d"]);
    }

    #[test]
    fn update_through_delta_event_shifts_aligned_layer_and_skips_others() {
        use crate::{Edit, Transaction};
        let mut buf = buffer("# zom 文档规范\n");
        let kind = MetadataLayerKind::custom("syntax");
        let other_kind = MetadataLayerKind::custom("other");

        let mut layers: MetadataLayers<&'static str> = MetadataLayers::new();
        layers
            .replace_layer_ranges(
                kind.clone(),
                buf.version(),
                vec![(range(0, 1), "punct"), (range(2, 18), "heading")],
            )
            .unwrap();
        layers.insert(
            other_kind.clone(),
            VersionedRangeSet::new(BufferVersion::new(buf.version().get() + 10)),
        );

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

        let set = layers.layer(&kind).unwrap();
        assert_eq!(set.version(), event.new_version());

        let mut ranges: Vec<(usize, usize)> = set
            .as_slice()
            .iter()
            .map(|entry| (entry.range().start().get(), entry.range().end().get()))
            .collect();
        ranges.sort();
        assert_eq!(ranges, vec![(0, 1), (2, 19)]);

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
            crate::errors::VersionedResultError::VersionMismatch { .. }
        ));
    }
}
