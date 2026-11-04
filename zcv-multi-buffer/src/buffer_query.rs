//! `MultiBuffer` 的快照、元信息与查询入口。
//!
//! 本文件的类型与字段仍定义在 crate 根模块 `multi_buffer.rs`。

use super::*;

impl MultiBuffer {
    /// 快照入口：源事件只置脏，读取时按源订阅批量推进组合状态。
    ///
    /// 组合快照由 MultiBuffer 持有。
    /// 普通源编辑只替换受影响源的源快照节点和受影响的 excerpts / diff transforms 树；
    /// 源集合或路径拓扑变化才重建源表。
    pub fn snapshot(&mut self, cx: &mut Context<Self>) -> MultiBufferSnapshot {
        self.sync_pending_sources(cx);
        self.finish_projection_sync(cx);
        self.sync_snapshot_from_state();
        self.snapshot.clone()
    }

    pub(crate) fn sync_pending_sources(&mut self, cx: &mut Context<Self>) {
        if self.state.pending_source_syncs.is_empty() {
            return;
        }
        let source_syncs = mem::take(&mut self.state.pending_source_syncs);
        for (source_id, sync) in source_syncs {
            self.synchronize_source_change(source_id, None, sync.refresh_metadata, cx);
        }
    }

    fn sync_snapshot_from_state(&mut self) {
        if !self.snapshot_dirty {
            return;
        }

        // `MultiBufferSnapshot` 同时拥有 excerpts、变换树和每个源的 `BufferSnapshot`。
        // 这里必须整帧替换：不能先更新源表、再等 excerpts 树在下一次读取时补齐，否则同一快照会把旧范围解析到新文本上。
        let source_snapshot = |source: &ExcerptSource| ExcerptSourceSnapshot {
            text: source.text.clone(),
            syntax: source.syntax.clone(),
            highlight_cache: Arc::clone(&source.highlight_cache),
            settings: Arc::clone(&source.settings),
            capture_map: Arc::clone(&source.capture_map),
            is_dirty: source.is_dirty,
        };
        let source_updates = self.snapshot_source_updates.take();
        let topology_changed = self.snapshot.topology_version != self.state.topology_version;
        let file_sources = if topology_changed {
            let mut files = TreeMap::default();
            for excerpt in self.state.excerpts.iter() {
                if files.get(&excerpt.buffer_id).is_some() {
                    files.update(&excerpt.buffer_id, |file: &mut FileSource| {
                        file.editable |= excerpt.editable
                    });
                } else {
                    files.insert(
                        excerpt.buffer_id,
                        FileSource {
                            path: excerpt.path.clone(),
                            source_index: excerpt.source_index,
                            editable: excerpt.editable,
                        },
                    );
                }
            }
            files
        } else {
            self.snapshot.file_sources.clone()
        };
        let dirty_source_count = match (topology_changed, source_updates.as_ref()) {
            (true, _) | (_, None) => file_sources
                .iter()
                .filter(|(_, file)| file.editable && self.state.sources[file.source_index].is_dirty)
                .count(),
            (false, Some(updates)) => {
                let mut count = self.snapshot.dirty_source_count;
                for &index in updates {
                    let source = &self.state.sources[index];
                    if file_sources
                        .get(&source.buffer_id)
                        .is_some_and(|file| file.editable)
                    {
                        let old_dirty = self
                            .snapshot
                            .excerpt_sources
                            .get(&index)
                            .expect("已有源必须在快照源表中")
                            .is_dirty;
                        count = count + usize::from(source.is_dirty) - usize::from(old_dirty);
                    }
                }
                count
            }
        };
        let (excerpt_sources, source_indices) = match source_updates {
            None => (
                TreeMap::from_ordered_entries(
                    self.state
                        .sources
                        .iter()
                        .enumerate()
                        .map(|(index, source)| (index, source_snapshot(source)))
                        .collect::<Vec<_>>(),
                ),
                Arc::new(
                    self.state
                        .sources
                        .iter()
                        .enumerate()
                        .map(|(index, source)| (source.entity.entity_id(), index))
                        .collect(),
                ),
            ),
            Some(updates) => {
                let mut excerpt_sources = self.snapshot.excerpt_sources.clone();
                for source_index in updates {
                    let source = &self.state.sources[source_index];
                    excerpt_sources
                        .update(&source_index, |current| *current = source_snapshot(source));
                }
                (excerpt_sources, Arc::clone(&self.snapshot.source_indices))
            }
        };
        // 在移动 source_indices 前解析单文件源索引。
        let singleton_source_index = self
            .singleton_source
            .as_ref()
            .and_then(|source| source_indices.get(&source.entity_id()).copied());
        self.snapshot = MultiBufferSnapshot {
            projection_version: self.state.projection_version,
            topology_version: self.state.topology_version,
            path_order: self.path_order,
            excerpts: self.state.excerpts.clone(),
            diff_transforms: self.state.diff_transforms.clone(),
            path_keys: if self.snapshot.path_keys.len() == self.state.path_keys.len() {
                Arc::clone(&self.snapshot.path_keys)
            } else {
                Arc::from(self.state.path_keys.clone())
            },
            excerpt_sources,
            source_indices,
            file_sources,
            dirty_source_count,
            capture_names: Arc::clone(&self.state.capture_names),
            metadata_version: self.state.metadata_epoch,
            diff_display: self.diff.clone(),
            singleton: self.singleton_source.is_some(),
            singleton_source_index,
        };
        self.snapshot_dirty = false;
        self.snapshot_source_updates = Some(HashSet::new());
    }

    /// 普通编辑器的工作区源（展开 diff 时作为新侧输入）。
    pub fn singleton_source(&self) -> Option<Entity<LanguageBuffer>> {
        self.singleton_source.clone()
    }

    /// 组合文档首个源的语言注册表；空组合文档返回 None。
    ///
    /// 需要创建关联语言 Buffer 的消费方复用本组合文档已经使用的注册表。
    pub fn language_registry(&self, cx: &App) -> Option<Arc<LanguageRegistry>> {
        self.state
            .sources
            .first()
            .map(|source| source.entity.read(cx).language_registry())
    }

    pub fn is_read_only(&self) -> bool {
        self.capability.is_read_only()
    }

    pub fn is_dirty(&mut self, cx: &mut Context<Self>) -> bool {
        self.snapshot(cx).is_dirty()
    }

    /// 文档实际引用的、可落盘的底层文件 Buffer，按源实体去重。
    pub fn file_buffers(&self, cx: &App) -> Vec<(Entity<LanguageBuffer>, PathBuf)> {
        let mut buffers = Vec::<(Entity<LanguageBuffer>, PathBuf)>::new();
        for entry in self.state.excerpts.iter().filter(|entry| entry.editable) {
            let source = self.state.sources[entry.source_index].entity.clone();
            let Some(path) = source.read(cx).file_path() else {
                continue;
            };
            if !buffers
                .iter()
                .any(|(existing, _)| existing.entity_id() == source.entity_id())
            {
                buffers.push((source, path));
            }
        }
        buffers
    }

    pub fn subscribe_and_snapshot(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (MultiBufferSubscription, MultiBufferSnapshot) {
        let snapshot = self.snapshot(cx);
        let subscription = self.state.projection_changes.subscribe(snapshot.version());
        (subscription, snapshot)
    }

    /// 无文件路径、内容也取不到标题时的回落文案。
    pub const DEFAULT_TITLE: &str = "untitled";

    /// 组合文档标题：文件路径文件名 → 单例内容首行 → [`Self::DEFAULT_TITLE`]。
    ///
    /// 标题归文档模型所有，展示层直接消费；返回值永不为空。
    pub fn title(&self, cx: &App) -> Cow<'_, str> {
        if let Some(path) = self.file_path(cx)
            && let Some(name) = path.file_name()
        {
            return name.to_string_lossy().into_owned().into();
        }

        if self.singleton_source.is_some()
            && let Some(title) = self.buffer_content_title(cx)
        {
            return title.into();
        }

        Self::DEFAULT_TITLE.into()
    }

    /// 从单例文档内容取标题：首行、折叠连续空白、最多 40 个字符、去尾部空白。
    ///
    /// 只在无文件路径时作为兜底。
    fn buffer_content_title(&self, cx: &App) -> Option<String> {
        let source = self.singleton_source.as_ref()?;
        let snapshot = source.read(cx).text_snapshot();
        let line = snapshot.slice_line(Line::ZERO).ok()?;

        let mut is_leading_whitespace = true;
        let mut count = 0;
        let mut prev_was_space = false;
        let mut title = String::new();

        for ch in line.as_str().chars() {
            if is_leading_whitespace && ch.is_whitespace() {
                continue;
            }

            is_leading_whitespace = false;

            if ch == '\n' || count >= 40 {
                break;
            }

            if ch.is_whitespace() {
                if !prev_was_space {
                    title.push(' ');
                    count += 1;
                    prev_was_space = true;
                }
            } else {
                title.push(ch);
                count += 1;
                prev_was_space = false;
            }
        }

        let title = title.trim_end().to_string();
        if title.is_empty() {
            return None;
        }
        Some(title)
    }

    pub fn file_path(&self, cx: &App) -> Option<PathBuf> {
        // 普通编辑器：从工作区源推导文件路径；
        // 无工作区源时退回第一个可编辑片段（DiffView 等组合视图）。
        self.singleton_source
            .as_ref()
            .and_then(|source| source.read(cx).file_path())
            .or_else(|| {
                self.state
                    .excerpts
                    .iter()
                    .filter(|entry| entry.editable)
                    .find_map(|entry| {
                        self.state.sources[entry.source_index]
                            .entity
                            .read(cx)
                            .file_path()
                    })
            })
    }

    /// 把当前组合偏移锚定到底层源坐标；affinity 决定同点插入的吸附方向。
    pub fn anchor_at(
        &self,
        offset: impl Into<MultiBufferOffset>,
        affinity: Affinity,
    ) -> MultiBufferAnchor {
        let offset: MultiBufferOffset = offset.into();
        anchor_in_mappings(
            &self.state.excerpts,
            &self.state.diff_transforms,
            Sources::Workspace(self.state.sources.as_slice()),
            offset.into(),
            affinity,
        )
        .unwrap_or_else(|| MultiBufferAnchor::boundary(offset))
    }

    /// 把稳定锚点定位到当前组合坐标。
    ///
    /// 源或 excerpt 退出当前投影时按路径顺序定位到相邻结构边界；
    /// 空投影定位到文首。源 Anchor 的版本链无法推进时返回错误。
    pub fn anchor_offset(&self, anchor: &MultiBufferAnchor) -> TextResult<MultiBufferOffset> {
        resolve_anchor_in_mappings(
            &self.state.excerpts,
            &self.state.diff_transforms,
            &self.state.path_keys,
            self.state.sources.as_slice(),
            &self.state.source_indices,
            anchor,
        )
        .map(|resolution| resolution.offset().into())
    }

    /// 只在锚点仍属于当前可见源片段时返回组合坐标。
    pub fn projected_anchor_offset(
        &self,
        anchor: &MultiBufferAnchor,
    ) -> TextResult<Option<MultiBufferOffset>> {
        resolve_anchor_in_mappings(
            &self.state.excerpts,
            &self.state.diff_transforms,
            &self.state.path_keys,
            self.state.sources.as_slice(),
            &self.state.source_indices,
            anchor,
        )
        .map(|resolution| resolution.projected_offset().map(Into::into))
    }

    /// 把组合文档中的选区映射回同一个源片段；跨片段选区没有单一源位置。
    pub fn location_for_range(&self, range: MultiBufferRange) -> Option<ExcerptLocation> {
        let (mapping, _) = mapping_at_tree(
            &self.state.excerpts,
            &self.state.diff_transforms,
            Sources::Workspace(self.state.sources.as_slice()),
            range.start().into(),
        )?;
        let starts_inside = range.start() >= mapping.output_range.start();
        let ends_inside = range.end() <= mapping.output_range.end();
        let empty_point_inside = !range.is_empty()
            || range.start() < mapping.output_range.end()
            || mapping.output_range.end()
                == MultiBufferOffset::new(self.state.diff_transforms.summary().output.len);
        if !(starts_inside && ends_inside && empty_point_inside) {
            return None;
        }
        let source_start = ByteOffset::new(
            (mapping.source_range.start().get()
                + range
                    .start()
                    .get()
                    .saturating_sub(mapping.output_range.start().get()))
            .min(mapping.source_range.end().get()),
        );
        let source_end = ByteOffset::new(
            (mapping.source_range.start().get()
                + range
                    .end()
                    .get()
                    .saturating_sub(mapping.output_range.start().get()))
            .min(mapping.source_range.end().get()),
        );
        Some(ExcerptLocation {
            path: mapping.path.as_path().to_path_buf(),
            source_range: TextRange::new(source_start, source_end).expect("源范围必须有效"),
        })
    }

    /// 当前 ordered excerpts 中真实内容匹配在组合坐标中的范围。
    ///
    /// 从 excerpt 树的权威 match_ranges 按需派生，不维护文档级扁平副本。
    pub fn match_ranges(&self) -> Vec<MultiBufferRange> {
        self.match_ranges_from_tree()
    }

    /// 更新工作区源的文件路径并重建投影（路径参与 excerpt 元数据与锚点解析）。
    pub fn set_file_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(source) = self.singleton_source.clone() else {
            return;
        };
        source.update(cx, |source, cx| source.set_file_path(path, cx));
        self.rebuild_display(cx);
    }

    /// `offset` 处所在 excerpt 的源语言名（组合文档按光标所在源文件显示语言）。
    pub fn language_at(&self, offset: MultiBufferOffset, cx: &App) -> Option<&'static str> {
        let offset: MultiBufferOffset = offset;
        let mapping = self.mapping_at(ByteOffset::new(offset.get()))?;
        self.state
            .sources
            .get(mapping.source_index)?
            .entity
            .read(cx)
            .language_name()
    }

    /// 定位组合偏移所属的映射；最后一个映射的结束偏移视为命中（光标位于文档末尾）。
    fn mapping_at(&self, offset: ByteOffset) -> Option<ExcerptMapping> {
        mapping_at_tree(
            &self.state.excerpts,
            &self.state.diff_transforms,
            Sources::Workspace(self.state.sources.as_slice()),
            offset,
        )
        .map(|(mapping, _)| mapping)
    }
}
