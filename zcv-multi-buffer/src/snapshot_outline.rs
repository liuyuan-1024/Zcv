//! `MultiBufferSnapshot` 的大纲条目投影。
//!
//! 本文件的类型与字段仍定义在 crate 根模块 `multi_buffer.rs`。

use super::*;

impl MultiBufferSnapshot {
    /// 返回当前组合文档中的大纲条目，含符号所属文件身份。
    ///
    /// 单文件文档直接取工作区源的完整语法大纲；组合文档只投影完整落在 excerpt 内的定义。
    /// 结果按显示路径与输出范围排序，供大纲面板构建「目录 → 文件 → 符号」树。
    pub fn outline_entries(&self) -> Vec<OutlineEntry> {
        let mut entries = match self.singleton_source_index {
            Some(source_index) => self.singleton_outline_entries(source_index),
            None => self.composite_outline_entries(),
        };
        entries.sort_unstable_by(|left, right| {
            left.display_path.cmp(&right.display_path).then_with(|| {
                (
                    self.anchor_offset(&left.item.range.start)
                        .expect("大纲锚点来自当前快照"),
                    self.anchor_offset(&left.item.range.end)
                        .expect("大纲锚点来自当前快照"),
                )
                    .cmp(&(
                        self.anchor_offset(&right.item.range.start)
                            .expect("大纲锚点来自当前快照"),
                        self.anchor_offset(&right.item.range.end)
                            .expect("大纲锚点来自当前快照"),
                    ))
            })
        });
        entries.dedup_by(|left, right| {
            left.display_path == right.display_path
                && left.item.range == right.item.range
                && left.item.name_range == right.item.name_range
                && left.item.name == right.item.name
                && left.item.language == right.item.language
        });
        entries
    }

    /// 单文件文档的大纲条目：直接取工作区源的完整语法大纲。
    ///
    /// 单文件不按 excerpt/diff 片段裁剪，源坐标只做单调映射到组合输出坐标，因此 diff 注入不会丢失跨 hunk 的定义。
    fn singleton_outline_entries(&self, source_index: usize) -> Vec<OutlineEntry> {
        let Some(source) = self.excerpt_sources.get(&source_index) else {
            return Vec::new();
        };
        let Some((display_path, path_index, source_id)) =
            self.source_outline_identity(source_index)
        else {
            return Vec::new();
        };
        let outlines = source.syntax.outline(
            0..source.text.len_bytes().get(),
            &source.text,
            &source.highlight_cache,
        );
        let segments = self.outline_segments(source_index);
        if segments.is_empty() {
            return Vec::new();
        }
        outlines
            .iter()
            .filter_map(|item| {
                let range = resolve_outline_range(&item.range, &source.text)?;
                project_range_contained(&segments, &range)
                    .or_else(|| project_range_spanning(&segments, &range))
                    .map(|_| OutlineEntry {
                        display_path: display_path.clone(),
                        item: self.anchor_source_outline_item(item, path_index, source_id),
                    })
            })
            .collect()
    }

    /// 组合文档的大纲条目：只投影完整落在某个可见 excerpt 内的定义。
    ///
    /// 搜索与项目差异使用窗口 excerpt，跨窗口的定义不展示；
    /// 窗口内定义仍按片段投影，并携带该片段的文件身份。
    fn composite_outline_entries(&self) -> Vec<OutlineEntry> {
        let source_outlines = self
            .excerpt_sources
            .values()
            .map(|source| {
                source.syntax.outline(
                    0..source.text.len_bytes().get(),
                    &source.text,
                    &source.highlight_cache,
                )
            })
            .collect::<Vec<_>>();
        let mut entries = Vec::new();
        let mut cursor = MultiBufferCursor::new(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
        );
        cursor.seek(ByteOffset::ZERO, Bias::Right);
        while let Some((excerpt, _)) = cursor.item() {
            let mapping = excerpt.to_mapping(cursor.start());
            let display_path = excerpt.display_path.as_path().to_path_buf();
            if let (Some(outlines), Some(source)) = (
                source_outlines.get(mapping.source_index),
                self.excerpt_sources.get(&mapping.source_index),
            ) {
                for item in outlines {
                    let anchored = if mapping.diff_kind == Some(ExcerptDiffKind::Deleted) {
                        let Some(projected) = project_outline_item(
                            item,
                            mapping.source_range.range(),
                            mapping.output_range,
                            &source.text,
                        ) else {
                            continue;
                        };
                        self.anchor_projected_outline_item(projected)
                    } else {
                        let Some(range) = resolve_outline_range(&item.range, &source.text) else {
                            continue;
                        };
                        if range.start < mapping.source_range.start().get()
                            || range.end > mapping.source_range.end().get()
                        {
                            continue;
                        }
                        self.anchor_source_outline_item(item, mapping.path_index, mapping.source_id)
                    };
                    entries.push(OutlineEntry {
                        display_path: display_path.clone(),
                        item: anchored,
                    });
                }
            }
            cursor.next();
        }
        entries
    }

    /// 工作区源符号直接保留源 Anchor 与片段身份，不从输出边界反推来源。
    fn anchor_source_outline_item(
        &self,
        item: &OutlineItem,
        path_index: PathKeyIndex,
        source_id: Option<EntityId>,
    ) -> OutlineItem<MultiBufferAnchor> {
        let wrap = |anchor: Anchor| MultiBufferAnchor::excerpt(path_index, source_id, anchor);
        let wrap_range = |range: &Range<Anchor>| wrap(range.start)..wrap(range.end);
        OutlineItem {
            range: wrap_range(&item.range),
            name_range: wrap_range(&item.name_range),
            source_range_for_text: wrap_range(&item.source_range_for_text),
            name: item.name.clone(),
            text: item.text.clone(),
            highlight_ranges: item.highlight_ranges.clone(),
            name_ranges: item.name_ranges.clone(),
            kind: item.kind.clone(),
            depth: item.depth,
            language: item.language,
            language_depth: item.language_depth,
            body_range: item.body_range.as_ref().map(wrap_range),
            annotation_range: item.annotation_range.as_ref().map(wrap_range),
        }
    }

    /// 删除侧需要同时保存工作区 hunk 身份和基线 Anchor，由投影入口构造该组合身份。
    fn anchor_projected_outline_item(
        &self,
        item: OutlineItem<usize>,
    ) -> OutlineItem<MultiBufferAnchor> {
        let anchor_range = |range: Range<usize>| {
            self.anchor_at(range.start, Affinity::After)
                ..self.anchor_at(range.end, Affinity::Before)
        };
        OutlineItem {
            range: anchor_range(item.range),
            name_range: anchor_range(item.name_range),
            source_range_for_text: anchor_range(item.source_range_for_text),
            name: item.name,
            text: item.text,
            highlight_ranges: item.highlight_ranges,
            name_ranges: item.name_ranges,
            kind: item.kind,
            depth: item.depth,
            language: item.language,
            language_depth: item.language_depth,
            body_range: item.body_range.map(anchor_range),
            annotation_range: item.annotation_range.map(anchor_range),
        }
    }

    /// 指定源在组合文档中的显示路径。
    fn source_outline_identity(
        &self,
        source_index: usize,
    ) -> Option<(PathBuf, PathKeyIndex, Option<EntityId>)> {
        let mut cursor = MultiBufferCursor::new(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
        );
        cursor.seek(ByteOffset::ZERO, Bias::Right);
        while let Some((excerpt, _)) = cursor.item() {
            let mapping = excerpt.to_mapping(cursor.start());
            if mapping.source_index == source_index {
                return Some((
                    excerpt.display_path.as_path().to_path_buf(),
                    mapping.path_index,
                    mapping.source_id,
                ));
            }
            cursor.next();
        }
        None
    }

    /// 指定源在组合文档中的可见片段（源坐标 -> 输出坐标），按输出顺序排列。
    fn outline_segments(&self, source_index: usize) -> Vec<(TextRange, MultiBufferRange)> {
        let mut segments = Vec::new();
        let mut cursor = MultiBufferCursor::new(
            &self.excerpts,
            &self.diff_transforms,
            Sources::Snapshot(&self.excerpt_sources),
        );
        cursor.seek(ByteOffset::ZERO, Bias::Right);
        while let Some((excerpt, _)) = cursor.item() {
            let mapping = excerpt.to_mapping(cursor.start());
            if mapping.source_index == source_index {
                segments.push((mapping.source_range.range(), mapping.output_range));
            }
            cursor.next();
        }
        segments
    }
}
