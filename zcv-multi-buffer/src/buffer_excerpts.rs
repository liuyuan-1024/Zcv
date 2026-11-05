//! `MultiBuffer` 的 excerpt 增删、路径装配与结构同步。
//!
//! 本文件的类型与字段仍定义在 crate 根模块 `multi_buffer.rs`。

use super::*;

impl MultiBuffer {
    /// 整篇重建组合文档的内部入口；只供 diff 投影重建与 clear 使用。
    ///
    /// 顺序权威归 MultiBuffer：内部按 PathKey 稳定排序后建树，调用方传入顺序不进入文档语义。
    pub(crate) fn replace_all_excerpts(
        &mut self,
        excerpts: Vec<ExcerptRange>,
        cx: &mut Context<Self>,
    ) {
        self.assert_no_active_transaction("MultiBuffer::replace_all_excerpts");
        self.mark_source_topology_changed();
        self.state.topology_version = self.state.topology_version.wrapping_add(1);
        let mut unique_sources = Vec::<Entity<LanguageBuffer>>::new();
        let mut unique_source_ids = HashSet::new();
        for excerpt in &excerpts {
            if unique_source_ids.insert(excerpt.source.entity_id()) {
                unique_sources.push(excerpt.source.clone());
            }
        }
        for file in &self.diffs {
            if let Some(base) = file.diff.read(cx).base_source()
                && unique_source_ids.insert(base.entity_id())
            {
                unique_sources.push(base.clone());
            }
        }
        // Deleted 源只由 BufferDiff 推进文本投影，但它的语法与语言元数据仍属于组合快照。
        let text_sources = excerpts
            .iter()
            .map(|excerpt| excerpt.source.entity_id())
            .collect::<HashSet<_>>();
        // 结构重建只替换拓扑：仍存活的源复用已有连接，不为一次重排重建全部订阅。
        let mut previous_subscriptions = mem::take(&mut self.state.source_subscriptions)
            .into_iter()
            .map(|subscription| (subscription.source.entity_id(), subscription))
            .collect::<HashMap<_, _>>();
        let mut next_source_subscriptions = Vec::with_capacity(unique_sources.len());
        for source in unique_sources {
            let source_id = source.entity_id();
            let track_text = text_sources.contains(&source_id);
            if let Some(subscription) = previous_subscriptions.remove(&source_id)
                && subscription.text.is_some() == track_text
            {
                // 新 excerpts 已采用当前源快照；
                // 丢弃复用订阅里已被快照吸收的待消费编辑，否则下一帧会按旧版本重放同一批编辑。
                if let Some(text) = &subscription.text {
                    text.consume();
                }
                next_source_subscriptions.push(subscription);
            } else {
                next_source_subscriptions.push(Self::subscribe_source(source, track_text, cx));
            }
        }
        // 折叠候选在源级缓存：结构重建时复用同一源已缓存的锚点，只有 reparse 才重算。
        let ExcerptState {
            source_subscriptions,
            excerpts: authoritative_excerpts,
            diff_transforms,
            sources,
            source_indices,
            path_keys,
            path_key_indices,
            capture_names: composite_capture_names,
            ..
        } = &mut self.state;

        // 按源去重构建 (text, syntax) 表：同一文件的大量片段共享一份源状态。
        let mut next_sources: Vec<ExcerptSource> = Vec::new();
        let mut next_source_indices = HashMap::new();
        struct PreparedExcerpt {
            excerpt: ExcerptRange,
            path: PathKey,
            source_index: usize,
            source_id: EntityId,
            start_line: usize,
        }
        let mut prepared = Vec::with_capacity(excerpts.len());
        for excerpt in excerpts {
            let source = excerpt.source.read(cx);
            let path = excerpt.path_key(self.path_order, cx);
            let file_path = excerpt.file_path(cx);
            let project_path = excerpt.project_path.clone();
            let buffer_id = source.buffer_id();
            let source_id = excerpt.source.entity_id();
            let source_index = match next_source_indices.get(&source_id).copied() {
                Some(index) => index,
                None => {
                    let snapshot = source.snapshot();
                    next_sources.push(ExcerptSource {
                        entity: excerpt.source.clone(),
                        path: path.clone(),
                        file_path: file_path.clone(),
                        project_path: project_path.clone(),
                        buffer_id,
                        settings: snapshot_settings(&snapshot),
                        text: snapshot.text,
                        syntax: snapshot.syntax,
                        highlight_cache: snapshot.highlight_cache,
                        capture_map: Arc::from([]),
                        is_dirty: snapshot.is_dirty,
                    });
                    let index = next_sources.len() - 1;
                    next_source_indices.insert(source_id, index);
                    index
                }
            };
            let source_snapshot = &next_sources[source_index];
            if !snapshot_range_is_valid(&source_snapshot.text, excerpt.source_range) {
                continue;
            }
            let start_line = source_snapshot
                .text
                .byte_to_line(excerpt.source_range.start())
                .map_or(0, |line| line.get());
            prepared.push(PreparedExcerpt {
                excerpt,
                path,
                source_index,
                source_id,
                start_line,
            });
        }

        for subscription in &next_source_subscriptions {
            let entity = &subscription.source;
            if let Entry::Vacant(entry) = next_source_indices.entry(entity.entity_id()) {
                let source = entity.read(cx);
                let snapshot = source.snapshot();
                let index = next_sources.len();
                entry.insert(index);
                next_sources.push(ExcerptSource {
                    entity: entity.clone(),
                    path: path_key_for_source(source, self.path_order),
                    file_path: source.file_path().unwrap_or_default(),
                    project_path: None,
                    buffer_id: source.buffer_id(),
                    settings: snapshot_settings(&snapshot),
                    text: snapshot.text,
                    syntax: snapshot.syntax,
                    highlight_cache: snapshot.highlight_cache,
                    capture_map: Arc::from([]),
                    is_dirty: snapshot.is_dirty,
                });
            }
        }
        // 组合文档的顺序由 PathKey 决定，不由调用方传入顺序决定：按路径稳定排序后建树。
        prepared.sort_by(|a, b| Ord::cmp(&a.path, &b.path));

        // 非末尾逻辑窗口各自拥有一个结构分隔换行，源文本已有的换行仍原样保留。
        // 分隔标志只随窗口结构变化，不随源文本编辑变化。
        let prepared_count = prepared.len();
        let mut next_excerpts = Vec::with_capacity(prepared_count);
        for (position, item) in prepared.into_iter().enumerate() {
            let display_path = item
                .excerpt
                .display_path
                .clone()
                .unwrap_or_else(|| item.path.clone());
            let Some((text_summary, _)) = snapshot_range_summary(
                &next_sources[item.source_index].text,
                item.excerpt.source_range,
            ) else {
                continue;
            };
            let adds_newline = position + 1 < prepared_count;
            let path_index = intern_path(path_keys, path_key_indices, &item.path);
            let excerpt = Excerpt {
                path: item.path,
                path_index,
                display_path,
                file_path: next_sources[item.source_index].file_path.clone(),
                buffer_id: next_sources[item.source_index].buffer_id,
                source_range: ExcerptContext::new(
                    &next_sources[item.source_index].text,
                    item.excerpt.source_range,
                    false,
                ),
                source_start_line: item.start_line,
                text_summary,
                adds_newline,
                match_ranges: Arc::from(item.excerpt.match_ranges),
                source_index: item.source_index,
                source_id: Some(item.source_id),
                editable: item.excerpt.editable,
            };
            next_excerpts.push(excerpt);
        }

        *source_subscriptions = next_source_subscriptions;
        *authoritative_excerpts = SumTree::from_iter(next_excerpts, ());
        *diff_transforms = SumTree::new(());
        *sources = next_sources;
        *source_indices = sources
            .iter()
            .enumerate()
            .map(|(index, source)| (source.entity.entity_id(), index))
            .collect();
        *composite_capture_names = rebuild_capture_table(sources);
        self.rebuild_all_diff_transforms(cx);
        self.emit_projection_changed(cx);
    }

    /// 为一组片段构建位置无关映射项；`start_index` 是它们在文档中的起始序号。
    fn build_entries_for_excerpts(
        &mut self,
        excerpts: Vec<ExcerptRange>,
        start_index: usize,
        total: usize,
    ) -> Vec<Excerpt> {
        let mut path_keys = mem::take(&mut self.state.path_keys);
        let mut path_key_indices = mem::take(&mut self.state.path_key_indices);
        let mut entries = Vec::with_capacity(excerpts.len());
        for (position, excerpt) in excerpts.into_iter().enumerate() {
            let source_id = excerpt.source.entity_id();
            let Some(&source_index) = self.state.source_indices.get(&source_id) else {
                continue;
            };
            let source = &self.state.sources[source_index];
            let path = source.path.clone();
            let display_path = excerpt.display_path.clone().unwrap_or_else(|| path.clone());
            let Some((text_summary, _)) =
                snapshot_range_summary(&source.text, excerpt.source_range)
            else {
                continue;
            };
            let adds_newline = start_index + position + 1 < total;
            let start_line = source
                .text
                .byte_to_line(excerpt.source_range.start())
                .map_or(0, |line| line.get());
            let path_index = intern_path(&mut path_keys, &mut path_key_indices, &path);
            entries.push(Excerpt {
                path,
                path_index,
                display_path,
                file_path: source.file_path.clone(),
                buffer_id: source.buffer_id,
                source_range: ExcerptContext::new(&source.text, excerpt.source_range, false),
                source_start_line: start_line,
                text_summary,
                adds_newline,
                match_ranges: Arc::from(excerpt.match_ranges),
                source_index,
                source_id: Some(source_id),
                editable: excerpt.editable,
            });
        }
        self.state.path_keys = path_keys;
        self.state.path_key_indices = path_key_indices;
        entries
    }

    /// 源编辑只推进逻辑 excerpt 的 Anchor、内容摘要与匹配范围。
    /// 分隔换行的生命周期由 excerpt 插入、移除维护。
    pub(crate) fn splice_source_path(
        &mut self,
        source_id: EntityId,
        source_position_map: &PositionMap,
        expanded_excerpts: Option<&HashSet<usize>>,
        _cx: &App,
    ) -> SourceEditRecords {
        let source_index = self.state.source_indices[&source_id];
        let source = &self.state.sources[source_index];
        let path = source.path.clone();
        let mut cursor = self.state.excerpts.cursor::<ExcerptSummary>(());
        let mut next = cursor.slice(&path, Bias::Left);
        let start_index = cursor.start().count;
        let mut old_input = cursor.start().text.len;
        let mut new_input = old_input;
        let removed = cursor.slice(&path, Bias::Right);
        let suffix = cursor.suffix();
        drop(cursor);
        let source_count = removed
            .iter()
            .filter(|entry| entry.source_id == Some(source_id))
            .count();
        let mut source_position = 0;
        let mut records = SourceEditRecords::default();
        let entries = removed
            .iter()
            .enumerate()
            .map(|(local, original)| {
                let mut entry = original.clone();
                if entry.source_id == Some(source_id) {
                    records.old.push(SourceExcerptRecord {
                        input_offset: ExcerptOffset::new(old_input),
                        source_range: entry.source_range.range(),
                    });
                    source_position += 1;
                    let (start_affinity, end_affinity) = match expanded_excerpts {
                        None => (
                            Affinity::Before,
                            if entry.source_range.is_empty() || source_position == source_count {
                                Affinity::After
                            } else {
                                Affinity::Before
                            },
                        ),
                        Some(expanded) if expanded.contains(&(start_index + local)) => {
                            (Affinity::Before, Affinity::After)
                        }
                        Some(_) => (Affinity::After, Affinity::Before),
                    };
                    entry.source_range = entry
                        .source_range
                        .mapped(&source.text, start_affinity, end_affinity)
                        .expect("源编辑必须属于 excerpt 的版本链");
                    entry.match_ranges = entry
                        .match_ranges
                        .iter()
                        .map(|range| {
                            source_position_map
                                .map_old_range_with_stickiness(*range, Stickiness::Never)
                                .value()
                        })
                        .collect::<Vec<_>>()
                        .into();
                    entry.text_summary =
                        snapshot_range_summary(&source.text, entry.source_range.range())
                            .expect("推进后的 excerpt 范围必须有效")
                            .0;
                    entry.source_start_line = source
                        .text
                        .byte_to_line(entry.source_range.start())
                        .expect("excerpt 起点必须属于源快照")
                        .get();
                    records.new.push(SourceExcerptRecord {
                        input_offset: ExcerptOffset::new(new_input),
                        source_range: entry.source_range.range(),
                    });
                }
                old_input += original.output_text_summary().len;
                new_input += entry.output_text_summary().len;
                entry
            })
            .collect::<Vec<_>>();
        next.extend(entries, ());
        next.append(suffix, ());
        self.state.excerpts = next;
        records
    }

    /// 显式替换一个路径的逻辑 excerpts，并把结构编辑交给统一变换同步器。
    fn splice_excerpt_entries(&mut self, path: &PathKey, entries: Vec<Excerpt>, cx: &App) {
        let before = self.projection_trees();
        let mut cursor = before.0.cursor::<ExcerptSummary>(());
        let mut next = cursor.slice(path, Bias::Left);
        let old_start = cursor.start().text.len;
        cursor.seek(path, Bias::Right);
        let old_end = cursor.start().text.len;
        let suffix = cursor.suffix();
        drop(cursor);
        next.extend(entries, ());
        next.append(suffix, ());
        let mut separator_edits = Vec::new();
        let old_tail = before.0.last();
        // 追加路径时，原文档尾获得一个结构分隔符。
        if let Some(old_tail) = old_tail
            && old_tail.path < *path
            && next.summary().count > before.0.summary().count
        {
            let mut cursor = next.cursor::<ExcerptSummary>(());
            let mut prefix = cursor.slice(&old_tail.path, Bias::Left);
            let mut previous_path = cursor.slice(&old_tail.path, Bias::Right);
            let input_start = prefix.summary().text.len;
            let footer_start = input_start + previous_path.summary().text.len;
            let suffix = cursor.suffix();
            drop(cursor);
            previous_path.update_last(|entry| entry.adds_newline = true, ());
            prefix.append(previous_path, ());
            prefix.append(suffix, ());
            next = prefix;
            separator_edits.push(diff_transform_sync::InputEdit::new(
                footer_start..footer_start,
                footer_start..footer_start + 1,
            ));
        }
        // 移除尾路径时，新的文档尾释放原有分隔符。
        if next.last().is_some_and(|entry| entry.adds_newline) {
            let old_footer = before.0.summary().text.len - (old_end - old_start) - 1;
            let new_footer = next.summary().text.len - 1;
            next.update_last(|entry| entry.adds_newline = false, ());
            separator_edits.push(diff_transform_sync::InputEdit::new(
                old_footer..old_footer + 1,
                new_footer..new_footer,
            ));
        }
        self.state.excerpts = next;
        let mut cursor = self.state.excerpts.cursor::<ExcerptSummary>(());
        cursor.seek(path, Bias::Left);
        let new_start = cursor.start().text.len;
        cursor.seek(path, Bias::Right);
        let new_end = cursor.start().text.len;
        drop(cursor);
        separator_edits.push(diff_transform_sync::InputEdit::new(
            old_start..old_end,
            new_start..new_end,
        ));
        self.sync_diff_transforms(&before, separator_edits, None, cx);
    }

    /// 从当前映射树派生组合坐标下的搜索匹配范围。
    ///
    /// excerpt 的 match_ranges 是权威数据；文档级列表按需派生，不另存可写副本。
    pub(crate) fn match_ranges_from_tree(&self) -> Vec<MultiBufferRange> {
        self.project_match_ranges(None)
    }

    fn match_ranges_for_path(&self, path: &PathKey) -> Vec<TextRange> {
        self.project_match_ranges(Some(path))
            .into_iter()
            .map(|range| {
                TextRange::new(range.start().into(), range.end().into()).expect("匹配范围必须正序")
            })
            .collect()
    }

    fn project_match_ranges(&self, path: Option<&PathKey>) -> Vec<MultiBufferRange> {
        let mut ranges = Vec::new();
        let mut cursor = MultiBufferCursor::new(
            &self.state.excerpts,
            &self.state.diff_transforms,
            Sources::Workspace(self.state.sources.as_slice()),
        );
        if let Some(path) = path {
            cursor.seek_path(path, Bias::Left);
        } else {
            cursor.seek(ByteOffset::ZERO, Bias::Right);
        }
        while let Some((excerpt, _)) = cursor.item() {
            if path.is_some_and(|path| excerpt.path != *path) {
                break;
            }
            let mapping = excerpt.to_mapping(cursor.start());
            let output_start = mapping.output_range.start().get();
            let source_start = mapping.source_range.start().get();
            for matched in mapping.match_ranges.iter() {
                if matched.start() < mapping.source_range.start()
                    || matched.end() > mapping.source_range.end()
                {
                    continue;
                }
                let start = output_start + matched.start().get() - source_start;
                let end = output_start + matched.end().get() - source_start;
                if let Ok(range) = MultiBufferRange::new(
                    MultiBufferOffset::new(start),
                    MultiBufferOffset::new(end),
                ) {
                    ranges.push(range);
                }
            }
            cursor.next();
        }
        ranges
    }

    /// 移除指定源路径的全部 excerpts；其余片段的组合坐标自动顺延。
    ///
    /// 只重算映射与匹配范围，不重建源订阅；路径不存在时返回 false。
    pub fn remove_excerpts_for_path(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        let path_key = PathKey::with_order(path.to_path_buf(), self.path_order);
        let removed = {
            let mut cursor = MultiBufferCursor::new(
                &self.state.excerpts,
                &self.state.diff_transforms,
                Sources::Workspace(self.state.sources.as_slice()),
            );
            cursor.seek_path(&path_key, Bias::Left);
            cursor
                .item()
                .is_some_and(|(entry, _)| entry.path == path_key)
        };
        if !removed {
            return false;
        }
        let before = self.projection_trees();
        let old_version = self.state.projection_version;
        self.state.topology_version = self.state.topology_version.wrapping_add(1);
        self.splice_excerpt_entries(&path_key, Vec::new(), cx);
        self.publish_projection_edit(&before, old_version);
        self.emit_projection_changed(cx);
        true
    }

    /// 注册尚未跟踪的源：文本/语法快照、变更订阅与捕获表；返回新增源的起始索引。
    pub(crate) fn register_sources(
        &mut self,
        new_sources: Vec<SourceRegistration>,
        subscribe_ids: &HashSet<EntityId>,
        cx: &mut Context<Self>,
    ) -> usize {
        if !new_sources.is_empty() {
            self.mark_source_topology_changed();
        }
        let first_new_source = self.state.sources.len();
        if new_sources.is_empty() {
            return first_new_source;
        }
        // 每个可见源都接收语法元数据事件；只有组合文本的权威输入订阅文本增量。
        let new_source_subscriptions = new_sources
            .iter()
            .map(|registration| {
                Self::subscribe_source(
                    registration.entity.clone(),
                    subscribe_ids.contains(&registration.entity.entity_id()),
                    cx,
                )
            })
            .collect::<Vec<_>>();
        self.state
            .sources
            .extend(new_sources.iter().map(|registration| {
                let language_buffer = registration.entity.read(cx);
                let snapshot = language_buffer.snapshot();
                let buffer_id = language_buffer.buffer_id();
                ExcerptSource {
                    entity: registration.entity.clone(),
                    path: registration.path.clone(),
                    file_path: language_buffer.file_path().unwrap_or_default(),
                    project_path: registration.project_path.clone(),
                    buffer_id,
                    settings: snapshot_settings(&snapshot),
                    text: snapshot.text,
                    syntax: snapshot.syntax,
                    highlight_cache: snapshot.highlight_cache,
                    capture_map: Arc::from([]),
                    is_dirty: snapshot.is_dirty,
                }
            }));
        self.state
            .source_subscriptions
            .extend(new_source_subscriptions);
        for (index, source) in self.state.sources.iter().enumerate().skip(first_new_source) {
            self.state
                .source_indices
                .insert(source.entity.entity_id(), index);
        }
        let mut capture_names = self.state.capture_names.iter().cloned().collect::<Vec<_>>();
        extend_capture_table(
            &mut self.state.sources,
            first_new_source,
            &mut capture_names,
        );
        self.state.capture_names = Arc::from(capture_names);
        first_new_source
    }

    /// 片段源文件路径；源必须已注册。
    fn excerpt_path(&self, excerpt: &ExcerptRange, cx: &App) -> PathKey {
        excerpt.path_key(self.path_order, cx)
    }

    /// 用给定 excerpts 替换其源路径现有的全部 excerpts（按路径有序插入）。
    ///
    /// 路径由片段的 ProjectPath 身份或源 file_path 确定；同一调用内的片段必须属于同一路径。
    /// 新增源自动注册，其余路径的片段与其组合坐标保持不变。
    /// 替换某路径的 excerpts，保持其余路径不变并维持路径升序。
    ///
    /// 返回该路径新片段在输出坐标中的匹配范围；调用方据此更新搜索高亮。
    pub fn set_excerpts_for_path(
        &mut self,
        excerpts: Vec<ExcerptRange>,
        cx: &mut Context<Self>,
    ) -> Vec<TextRange> {
        self.assert_no_active_transaction("MultiBuffer::set_excerpts_for_path");
        self.sync_pending_sources(cx);
        if excerpts.is_empty() {
            return Vec::new();
        }
        let mut new_sources = Vec::new();
        let mut seen_source_ids = HashSet::new();
        for excerpt in &excerpts {
            let source_id = excerpt.source.entity_id();
            if !self.state.source_indices.contains_key(&source_id)
                && seen_source_ids.insert(source_id)
            {
                new_sources.push(SourceRegistration {
                    entity: excerpt.source.clone(),
                    path: self.excerpt_path(excerpt, cx),
                    project_path: excerpt.project_path.clone(),
                });
            }
        }
        let subscribe_ids = excerpts
            .iter()
            .map(|excerpt| excerpt.source.entity_id())
            .collect::<HashSet<_>>();
        self.register_sources(new_sources, &subscribe_ids, cx);
        let path = self.excerpt_path(&excerpts[0], cx);
        debug_assert!(
            excerpts
                .iter()
                .all(|excerpt| self.excerpt_path(excerpt, cx) == path),
            "set_excerpts_for_path 的片段必须属于同一路径"
        );
        self.apply_excerpts_for_path(path, excerpts, cx)
    }

    /// 按路径顺序把多个新文件追加到组合文档，并以一次结构增量发布。
    /// 调用方必须保证新路径严格位于现有路径之后；返回范围可直接追加到搜索装饰。
    pub fn append_excerpts_for_paths(
        &mut self,
        paths: Vec<Vec<ExcerptRange>>,
        cx: &mut Context<Self>,
    ) -> AppendedExcerpts {
        self.assert_no_active_transaction("MultiBuffer::append_excerpts_for_paths");
        self.sync_pending_sources(cx);
        let before = self.projection_trees();
        let base_version = self.state.projection_version;
        let mut previous_path = self.state.excerpts.last().map(|entry| entry.path.clone());
        let mut new_sources = Vec::new();
        let mut subscribe_ids = HashSet::new();
        let mut flattened = Vec::new();
        let mut paths_to_project = Vec::with_capacity(paths.len());
        for group in paths {
            let first = group.first().expect("追加的文件必须包含片段");
            let path = first.path_key(self.path_order, cx);
            assert!(
                previous_path
                    .as_ref()
                    .is_none_or(|previous| previous < &path),
                "追加路径必须按组合文档路径顺序严格递增"
            );
            assert!(
                group
                    .iter()
                    .all(|excerpt| excerpt.path_key(self.path_order, cx) == path),
                "同一组片段必须属于同一路径"
            );
            previous_path = Some(path.clone());
            paths_to_project.push(path);
            for excerpt in group {
                let source_id = excerpt.source.entity_id();
                if subscribe_ids.insert(source_id)
                    && !self.state.source_indices.contains_key(&source_id)
                {
                    new_sources.push(SourceRegistration {
                        entity: excerpt.source.clone(),
                        path: excerpt.path_key(self.path_order, cx),
                        project_path: excerpt.project_path.clone(),
                    });
                }
                flattened.push(excerpt);
            }
        }
        if flattened.is_empty() {
            return AppendedExcerpts {
                base_version,
                version: base_version,
                match_ranges: Vec::new(),
            };
        }
        self.register_sources(new_sources, &subscribe_ids, cx);
        let old_count = self.state.excerpts.summary().count;
        let added_count = flattened.len();
        let mut entries =
            self.build_entries_for_excerpts(flattened, old_count, old_count + added_count);
        if entries.is_empty() {
            return AppendedExcerpts {
                base_version,
                version: base_version,
                match_ranges: Vec::new(),
            };
        }
        for entry in &mut entries {
            entry.adds_newline = true;
        }
        entries.last_mut().expect("追加的片段必须有效").adds_newline = false;
        self.state.topology_version = self.state.topology_version.wrapping_add(1);
        let old_len = before.0.summary().text.len;
        let mut next = before.0.clone();
        if next.last().is_some() {
            next.update_last(|entry| entry.adds_newline = true, ());
        }
        next.extend(entries, ());
        let new_len = next.summary().text.len;
        self.state.excerpts = next;
        self.sync_diff_transforms(
            &before,
            vec![diff_transform_sync::InputEdit::new(
                old_len..old_len,
                old_len..new_len,
            )],
            None,
            cx,
        );
        self.publish_projection_edit(&before, base_version);
        if self.projection_sync.is_none() {
            cx.emit(MultiBufferEvent::ProjectionAppended);
            cx.notify();
        }
        let match_ranges = paths_to_project
            .iter()
            .flat_map(|path| self.project_match_ranges(Some(path)))
            .collect::<Vec<_>>();
        AppendedExcerpts {
            base_version,
            version: self.state.projection_version,
            match_ranges,
        }
    }

    /// 用给定片段替换某路径的 excerpts，保持其余路径不变并维持路径升序。
    fn apply_excerpts_for_path(
        &mut self,
        path: PathKey,
        excerpts: Vec<ExcerptRange>,
        cx: &mut Context<Self>,
    ) -> Vec<TextRange> {
        let before = self.projection_trees();
        let old_version = self.state.projection_version;
        self.state.topology_version = self.state.topology_version.wrapping_add(1);
        let (start_index, old_count) = {
            let mut cursor = self.state.excerpts.cursor::<ExcerptSummary>(());
            cursor.seek(&path, Bias::Left);
            let start = cursor.start().count;
            cursor.seek(&path, Bias::Right);
            (start, cursor.start().count - start)
        };
        let total = self.state.excerpts.summary().count - old_count + excerpts.len();
        let entries = self.build_entries_for_excerpts(excerpts, start_index, total);
        self.splice_excerpt_entries(&path, entries, cx);
        let match_ranges = self.match_ranges_for_path(&path);
        self.publish_projection_edit(&before, old_version);
        self.emit_projection_changed(cx);
        match_ranges
    }
}
