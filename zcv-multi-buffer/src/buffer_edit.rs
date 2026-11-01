//! `MultiBuffer` 的编辑、事务与撤销／重做。
//!
//! 本文件的类型与字段仍定义在 crate 根模块 `multi_buffer.rs`。

use super::*;

impl MultiBuffer {
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.assert_no_active_transaction("MultiBuffer::clear");
        let before = self.projection_trees();
        let old_version = self.state.projection_version;
        self.replace_all_excerpts(Vec::new(), cx);
        self.publish_projection_edit(&before, old_version);
    }

    /// 将 MultiBuffer 坐标中的编辑拆分到各个底层 Buffer。
    ///
    /// 组合文本只通过映射与游标读取，不创建可变的第二份逻辑文档。
    /// 同一 excerpt 直接映射；跨 excerpt 替换只在起始 excerpt 插入新文本，并删除起始尾段、中间 excerpt 和结束首段。
    ///
    /// 编辑本身不返回投影重映射：选区以源 Anchor 为唯一权威，调用方在编辑后按当前快照重新锚定即可。
    pub fn edit(
        &mut self,
        edits: Vec<Edit>,
        metadata: TransactionMetadata,
        cx: &mut Context<Self>,
    ) -> TextResult<()> {
        if self.capability.is_read_only() {
            return Err(StorageError::ReadOnly.into());
        }

        // 源实体按 source_index 稳定索引；供闭包查源而不借用整个 state。
        let source_entities = self
            .state
            .sources
            .iter()
            .map(|source| source.entity.clone())
            .collect::<Vec<_>>();
        // 映射的源范围只在建立映射的那份源快照上有效；提交前用它校验源没有被并发推进。
        let source_versions = self
            .state
            .sources
            .iter()
            .map(|source| source.text.version())
            .collect::<Vec<_>>();

        let mut grouped: Vec<(Entity<LanguageBuffer>, BufferVersion, Vec<Edit>)> = Vec::new();
        let mut edited_excerpts = HashSet::new();
        let push_source_edit =
            |mapping: &ExcerptMapping,
             source_range: TextRange,
             replacement: String,
             grouped: &mut Vec<(Entity<LanguageBuffer>, BufferVersion, Vec<Edit>)>,
             edited_excerpts: &mut HashSet<usize>| {
                edited_excerpts.insert(mapping.excerpt_index);
                let source = source_entities[mapping.source_index].clone();
                let source_edit = Edit::replace(source_range, replacement);
                if let Some((_, _, source_edits)) = grouped
                    .iter_mut()
                    .find(|(candidate, _, _)| candidate.entity_id() == source.entity_id())
                {
                    source_edits.push(source_edit);
                } else {
                    grouped.push((
                        source,
                        source_versions[mapping.source_index],
                        vec![source_edit],
                    ));
                }
            };
        for edit in edits {
            let range = edit.range();
            let start_mapping = mapping_at_tree(
                &self.state.excerpts,
                &self.state.diff_transforms,
                Sources::Workspace(self.state.sources.as_slice()),
                range.start(),
            )
            .map(|(mapping, _)| mapping)
            .ok_or_else(|| TextError::InvariantViolation {
                location: "MultiBuffer::edit",
                detail: "编辑起点不在可见 excerpt 中".to_string(),
            })?;
            let mappings = mappings_in_output_range(
                &self.state.excerpts,
                &self.state.diff_transforms,
                Sources::Workspace(self.state.sources.as_slice()),
                start_mapping,
                range.end(),
            );
            if mappings.iter().any(|mapping| !mapping.editable) {
                return Err(StorageError::ReadOnly.into());
            }
            let start_mapping = mappings.first().expect("编辑起始 excerpt 必须存在");
            let end_mapping = mappings.last().expect("编辑终止 excerpt 必须存在");
            let source_start = ByteOffset::new(
                (start_mapping.source_range.start().get()
                    + range
                        .start()
                        .get()
                        .saturating_sub(start_mapping.output_range.start().get()))
                .min(start_mapping.source_range.end().get()),
            );
            let source_end = ByteOffset::new(
                (end_mapping.source_range.start().get()
                    + range
                        .end()
                        .get()
                        .saturating_sub(end_mapping.output_range.start().get()))
                .min(end_mapping.source_range.end().get()),
            );
            if mappings.len() == 1 {
                push_source_edit(
                    start_mapping,
                    TextRange::new(source_start, source_end).expect("已验证的源范围必须有效"),
                    edit.replacement().to_owned(),
                    &mut grouped,
                    &mut edited_excerpts,
                );
            } else {
                push_source_edit(
                    start_mapping,
                    TextRange::new(source_start, start_mapping.source_range.end())
                        .expect("起始 excerpt 尾段必须有效"),
                    edit.replacement().to_owned(),
                    &mut grouped,
                    &mut edited_excerpts,
                );
                for mapping in mappings
                    .iter()
                    .skip(1)
                    .take(mappings.len().saturating_sub(2))
                {
                    push_source_edit(
                        mapping,
                        mapping.source_range.range(),
                        String::new(),
                        &mut grouped,
                        &mut edited_excerpts,
                    );
                }
                push_source_edit(
                    end_mapping,
                    TextRange::new(end_mapping.source_range.start(), source_end)
                        .expect("结束 excerpt 首段必须有效"),
                    String::new(),
                    &mut grouped,
                    &mut edited_excerpts,
                );
            }
        }

        // 预检：任一源在映射建立后推进过版本，就说明源范围不再对应当前文本。
        // 必须在任何源提交前一次性判定，否则先提交的源会留下部分提交文本（单个源 Buffer 的提交是原子的，跨源提交不是）。
        for (source, base_version, _) in &grouped {
            let actual = source.read(cx).version();
            if actual != *base_version {
                return Err(TransactionError::VersionMismatch {
                    expected: *base_version,
                    actual,
                }
                .into());
            }
            // 只读源同样必须在任何源提交前拒绝，否则先提交的源会留下部分提交文本。
            if source.read(cx).is_read_only() {
                return Err(StorageError::ReadOnly.into());
            }
        }

        let mut edited_source_ids = Vec::with_capacity(grouped.len());
        for (source, _, source_edits) in grouped {
            source.update(cx, |source, cx| {
                source.edit(source_edits, metadata.clone(), cx)
            })?;
            edited_source_ids.push(source.entity_id());
        }

        // 组合编辑写回工作区源后，立即从 MultiBuffer 拥有的订阅拉取同一批变化。
        // 稍后到达的 LanguageBuffer 事件只负责唤醒，不再携带或重放增量。
        // hunk 变化由 BufferDiffEvent::DiffChanged 异步驱动物化；
        // 本轮回传的映射即编辑后、重物化前的坐标系，选区落位不依赖 diff 重建时机。
        for source_id in edited_source_ids {
            self.synchronize_source_change(source_id, Some(&edited_excerpts), false, cx)
                .ok_or_else(|| TextError::InvariantViolation {
                    location: "MultiBuffer::edit",
                    detail: "源 Buffer 已提交编辑但 MultiBuffer 订阅未收到变化".to_string(),
                })?;
        }
        Ok(())
    }

    /// 源文件路径变化后，按各源当前路径重建映射项（低频操作，允许整体重排）。
    pub(crate) fn rebuild_display(&mut self, cx: &mut Context<Self>) {
        self.state.topology_version = self.state.topology_version.wrapping_add(1);
        let before = self.projection_trees();
        let old_version = self.state.projection_version;
        let mut path_keys = mem::take(&mut self.state.path_keys);
        let mut path_key_indices = mem::take(&mut self.state.path_key_indices);
        let mut entries = self.state.excerpts.iter().cloned().collect::<Vec<_>>();
        for excerpt in &mut entries {
            let source = &mut self.state.sources[excerpt.source_index];
            let path = path_key_for_source(source.entity.read(cx), self.path_order);
            source.path = path.clone();
            excerpt.path = path.clone();
            excerpt.display_path = path.clone();
            excerpt.path_index = intern_path(&mut path_keys, &mut path_key_indices, &path);
        }
        entries.sort_by(|a, b| Ord::cmp(&a.path, &b.path));
        let total = entries.len();
        for (index, excerpt) in entries.iter_mut().enumerate() {
            excerpt.adds_newline = index + 1 < total;
        }
        self.state.path_keys = path_keys;
        self.state.path_key_indices = path_key_indices;
        self.state.excerpts = SumTree::from_iter(entries, ());
        self.rebuild_all_diff_transforms(cx);
        self.publish_projection_edit(&before, old_version);
        self.emit_projection_changed(cx);
    }

    pub fn start_transaction(&mut self, cx: &mut Context<Self>) -> TextResult<TransactionId> {
        if self.history_owner() == HistoryOwner::SourceBuffer {
            let source = self
                .singleton_source
                .as_ref()
                .expect("共享源历史必须有工作区源");
            return source
                .update(cx, |source, _| source.start_transaction())?
                .ok_or_else(|| TextError::InvariantViolation {
                    location: "MultiBuffer::start_transaction",
                    detail: "工作区源 Buffer 已有活动事务".to_string(),
                });
        }
        // 先收集需要开启底层事务的源，避免与后续对 state 的可变借用冲突。
        let sources = {
            let mut cursor = MultiBufferCursor::new(
                &self.state.excerpts,
                &self.state.diff_transforms,
                Sources::Workspace(self.state.sources.as_slice()),
            );
            cursor.seek(ByteOffset::ZERO, Bias::Right);
            let mut sources = Vec::new();
            while let Some((excerpt, _)) = cursor.item() {
                if excerpt.editable {
                    sources.push(self.state.sources[excerpt.source_index].entity.clone());
                }
                cursor.next();
            }
            sources
        };
        let ExcerptState {
            next_transaction_id,
            active_transaction,
            active_source_transactions,
            ..
        } = &mut self.state;
        if active_transaction.is_some() {
            return Err(TextError::InvariantViolation {
                location: "MultiBuffer::start_transaction",
                detail: "MultiBuffer 不允许嵌套事务".to_string(),
            });
        }
        *next_transaction_id =
            next_transaction_id
                .next()
                .ok_or_else(|| TextError::InvariantViolation {
                    location: "MultiBuffer::start_transaction",
                    detail: "MultiBuffer 事务 ID 溢出".to_string(),
                })?;
        let id = *next_transaction_id;
        for source in sources {
            if active_source_transactions
                .iter()
                .any(|candidate| candidate.entity_id() == source.entity_id())
            {
                continue;
            }
            source
                .update(cx, |source, _| source.start_transaction())?
                .ok_or_else(|| TextError::InvariantViolation {
                    location: "MultiBuffer::start_transaction",
                    detail: "excerpt 底层 Buffer 已有活动事务".to_string(),
                })?;
            active_source_transactions.push(source);
        }
        *active_transaction = Some(id);
        Ok(id)
    }

    pub fn end_transaction(&mut self, cx: &mut Context<Self>) -> Option<TransactionId> {
        if self.history_owner() == HistoryOwner::SourceBuffer {
            let source = self
                .singleton_source
                .as_ref()
                .expect("共享源历史必须有工作区源");
            return source
                .update(cx, |source, _| source.end_transaction())
                .ok()
                .flatten();
        }
        let ExcerptState {
            active_transaction,
            active_source_transactions,
            undo_stack,
            redo_stack,
            ..
        } = &mut self.state;
        let id = active_transaction.take()?;
        let mut transactions = Vec::new();
        for source in active_source_transactions.drain(..) {
            if let Some(transaction_id) = source
                .update(cx, |source, _| source.end_transaction())
                .ok()
                .flatten()
            {
                transactions.push((source, transaction_id));
            }
        }
        if transactions.is_empty() {
            return None;
        }
        // 源历史合并进前一节点（如输入法组合的 MergeWithPrevious）时，组合历史同样并入前一条目，保持「一次组合会话 = 一个撤销步」；
        // 此时返回被并入条目的身份，宿主据此清理本次会话的孤儿选区记录。
        if let Some(previous) = undo_stack.last()
            && previous.buffers.len() == transactions.len()
            && previous.buffers.iter().all(|(buffer, previous_id)| {
                transactions.iter().any(|(candidate, candidate_id)| {
                    candidate.entity_id() == buffer.entity_id() && candidate_id == previous_id
                })
            })
        {
            redo_stack.clear();
            return Some(previous.id);
        }
        undo_stack.push(CompositeHistoryEntry {
            id,
            buffers: transactions,
        });
        redo_stack.clear();
        Some(id)
    }

    /// 当前历史节点的组合事务身份；无历史时为 `None`。
    ///
    /// 撤销后回退到前一条目，编辑合并进前节点时保持不变，与源 Buffer 的当前节点语义一致。
    pub fn current_history_transaction(&self, cx: &App) -> Option<TransactionId> {
        if self.history_owner() == HistoryOwner::SourceBuffer {
            let source = self
                .singleton_source
                .as_ref()
                .expect("共享源历史必须有工作区源");
            return source.read(cx).current_history_transaction_id();
        }
        self.state.undo_stack.last().map(|entry| entry.id)
    }

    pub fn undo(
        &mut self,
        cx: &mut Context<Self>,
    ) -> TextResult<Option<MultiBufferHistoryOutcome>> {
        self.replay_history(false, cx)
    }

    pub fn redo(
        &mut self,
        cx: &mut Context<Self>,
    ) -> TextResult<Option<MultiBufferHistoryOutcome>> {
        self.replay_history(true, cx)
    }

    fn replay_history(
        &mut self,
        redo: bool,
        cx: &mut Context<Self>,
    ) -> TextResult<Option<MultiBufferHistoryOutcome>> {
        if self.history_owner() == HistoryOwner::SourceBuffer {
            let source = self
                .singleton_source
                .clone()
                .expect("共享源历史必须有工作区源");
            let old_version = self.snapshot(cx).version();
            let outcome = source.update(cx, |source, cx| {
                if redo {
                    source.redo(cx)
                } else {
                    source.undo(cx)
                }
            })?;
            let Some(outcome) = outcome else {
                return Ok(None);
            };
            let source_change = self
                .synchronize_source_change(source.entity_id(), None, false, cx)
                .ok_or_else(|| TextError::InvariantViolation {
                    location: "MultiBuffer::replay_history",
                    detail: "源 Buffer 已回放历史但 MultiBuffer 订阅未收到变化".to_string(),
                })?;
            let source_map = source_change.position_map();
            return Ok(Some(MultiBufferHistoryOutcome {
                transaction_id: outcome.transaction_id(),
                // 回放结果必须携带源事务的坐标映射：
                // 调用方用它推进锚定在投影坐标上的派生状态（自动闭合区域等）。
                // 空映射会让这些锚点停留在旧偏移。
                position_map: source_map.clone(),
                old_version,
                new_version: self.snapshot(cx).version(),
            }));
        }
        let (entry, old_version) = {
            let ExcerptState {
                undo_stack,
                redo_stack,
                ..
            } = &mut self.state;
            let entry = if redo {
                redo_stack.pop()
            } else {
                undo_stack.pop()
            };
            let Some(entry) = entry else {
                return Ok(None);
            };
            (entry, self.snapshot(cx).version())
        };

        let mut replayed_source_ids = Vec::new();
        for (buffer, expected_transaction) in &entry.buffers {
            if !redo
                && buffer
                    .read(cx)
                    .current_history_transaction_id()
                    .is_none_or(|transaction_id| transaction_id != *expected_transaction)
            {
                return Err(TextError::InvariantViolation {
                    location: "MultiBuffer::undo",
                    detail: "底层 Buffer 历史已在 MultiBuffer 外部分叉".to_string(),
                });
            }
            let mut cursor = MultiBufferCursor::new(
                &self.state.excerpts,
                &self.state.diff_transforms,
                Sources::Workspace(self.state.sources.as_slice()),
            );
            cursor.seek(ByteOffset::ZERO, Bias::Right);
            let source = loop {
                let Some((excerpt, _)) = cursor.item() else {
                    break None;
                };
                if self.state.sources[excerpt.source_index].entity.entity_id() == buffer.entity_id()
                {
                    break Some(self.state.sources[excerpt.source_index].entity.clone());
                }
                cursor.next();
            };
            let Some(source) = source else {
                return Err(TextError::InvariantViolation {
                    location: "MultiBuffer::replay_history",
                    detail: "历史 Buffer 不再属于当前组合文档源".to_string(),
                });
            };
            let outcome = source.update(cx, |source, cx| {
                if redo {
                    source.redo(cx)
                } else {
                    source.undo(cx)
                }
            })?;
            if outcome.is_none() {
                return Err(TextError::InvariantViolation {
                    location: "MultiBuffer::replay_history",
                    detail: "底层 Buffer 缺少对应的历史节点".to_string(),
                });
            }
            // 历史条目记录的是 excerpt 源的内层 Buffer；
            // source id 与 diff 文件（按 working LanguageBuffer 索引）和 excerpt 源统一身份口径。
            let source_id = source.entity_id();
            replayed_source_ids.push(source_id);
        }
        for source_id in replayed_source_ids {
            self.synchronize_source_change(source_id, None, false, cx)
                .ok_or_else(|| TextError::InvariantViolation {
                    location: "MultiBuffer::replay_history",
                    detail: "源 Buffer 已回放历史但 MultiBuffer 订阅未收到变化".to_string(),
                })?;
        }
        let position_map = PositionMap::default();
        let new_version = self.snapshot(cx).version();
        let transaction_id = entry.id;
        if redo {
            self.state.undo_stack.push(entry);
        } else {
            self.state.redo_stack.push(entry);
        }
        Ok(Some(MultiBufferHistoryOutcome {
            transaction_id,
            position_map,
            old_version,
            new_version,
        }))
    }
}
