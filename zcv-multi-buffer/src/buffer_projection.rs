//! `MultiBuffer` 的投影发布与同步：变更通知、投影事务与源订阅。
//!
//! 本文件的类型与字段仍定义在 crate 根模块 `multi_buffer.rs`。

use super::*;

impl MultiBuffer {
    /// 登记一段投影变化：同步帧内只累积净编辑，帧末一次性发布；帧外立即发布。
    pub(crate) fn publish_projection_change(&mut self, incremental: SourceIncremental) {
        self.mark_snapshot_changed();
        if let Some(sync) = &mut self.projection_sync {
            // 累积精确输出增量，帧末统一重定基到投影版本；不以树比较粗范围替代。
            sync.changes = sync.changes.compose_projection_edits(&incremental.batch);
            sync.changed = true;
            return;
        }
        self.publish_projection_change_now(incremental);
    }

    fn publish_projection_change_now(&mut self, incremental: SourceIncremental) {
        let old_version = self.state.projection_version;
        let new_version = old_version
            .next()
            .expect("组合投影版本不应溢出；溢出时必须创建新文档生命周期");
        self.state.projection_version = new_version;
        let batch = incremental.batch.rebased_to(old_version, new_version);
        self.state
            .projection_changes
            .publish(old_version, new_version, batch);
    }

    /// 开始一个把源快照和 diff 投影一起推进的同步帧。
    pub(crate) fn begin_projection_sync(&mut self) {
        if self.projection_sync.is_none() {
            self.projection_sync = Some(ProjectionSync {
                changes: TextChangeBatch::default(),
                changed: false,
                snapshot_changed: false,
                diff_changed: false,
                waiting_for_diff_sources: HashSet::new(),
            });
        }
    }

    /// 提交同步帧，使源映射、diff transform 和投影版本同时对外可见。
    pub(crate) fn finish_projection_sync(&mut self, cx: &mut Context<Self>) {
        let Some(sync) = self.projection_sync.as_ref() else {
            return;
        };
        if sync
            .waiting_for_diff_sources
            .iter()
            .any(|source_id| !self.diff_source_sync_ready(*source_id, cx))
        {
            return;
        }
        let sync = self.projection_sync.take().expect("同步帧在检查后仍应存在");
        if !sync.changed {
            if sync.snapshot_changed {
                cx.notify();
            }
        } else {
            self.publish_projection_change_now(SourceIncremental {
                batch: sync.changes,
            });
            self.emit_projection_changed(cx);
        }
        if sync.diff_changed {
            cx.emit(MultiBufferEvent::DiffChanged);
            cx.notify();
        }
    }

    pub(crate) fn emit_text_changed(&mut self, cx: &mut Context<Self>) {
        if self.projection_sync.is_none() {
            cx.emit(MultiBufferEvent::TextChanged);
            cx.notify();
        }
    }

    pub(crate) fn emit_projection_changed(&mut self, cx: &mut Context<Self>) {
        if self.projection_sync.is_none() {
            cx.emit(MultiBufferEvent::ProjectionChanged);
            cx.notify();
        }
    }

    pub(crate) fn notify_if_not_syncing(&mut self, cx: &mut Context<Self>) {
        if let Some(sync) = &mut self.projection_sync {
            sync.snapshot_changed = true;
        } else {
            cx.notify();
        }
    }

    /// 用编辑前冻结的投影树与当前投影树按游标推导结构变化范围，并发布增量批次。
    ///
    /// 结构变化不物化组合文本：前后两棵树按同版本源片段比对公共前后缀，不把 hunk 装饰造成的节点拆分当成文本编辑。
    /// 源内部编辑由 TextChangeBatch 单独投影。
    pub(crate) fn publish_projection_edit(
        &mut self,
        before: &ProjectionTrees,
        old_version: BufferVersion,
    ) {
        let after = self.projection_trees();
        let (old_range, new_range) = projection_changed_ranges(
            before,
            &after,
            Sources::Workspace(self.state.sources.as_slice()),
        );
        let batch =
            TextChangeBatch::from_edits(old_version, old_version, vec![(old_range, new_range)]);
        self.publish_projection_change(SourceIncremental { batch });
    }

    /// 冻结当前输入/输出投影树；供结构变化前保存旧坐标、变化后推导增量范围。
    pub(crate) fn projection_trees(&self) -> ProjectionTrees {
        (
            self.state.excerpts.clone(),
            self.state.diff_transforms.clone(),
        )
    }

    /// 把源版本化编辑投影到稳定逻辑 excerpts 的输入坐标。
    pub(crate) fn source_input_edits(
        &self,
        source_change: &TextChangeBatch,
        records: &SourceEditRecords,
    ) -> Vec<diff_transform_sync::InputEdit> {
        let mut edits = Vec::new();
        for (old, new) in records.old.iter().zip(&records.new) {
            for edit in source_change.patch().edits() {
                let start = edit.old_range().start().max(old.source_range.start());
                let end = edit.old_range().end().min(old.source_range.end());
                if start > end {
                    continue;
                }
                let new_start = edit.new_range().start().max(new.source_range.start());
                let new_end = edit.new_range().end().min(new.source_range.end());
                if new_start > new_end {
                    continue;
                }
                let old_range = (old.input_offset.get() + start.get()
                    - old.source_range.start().get())
                    ..(old.input_offset.get() + end.get() - old.source_range.start().get());
                let new_range = (new.input_offset.get() + new_start.get()
                    - new.source_range.start().get())
                    ..(new.input_offset.get() + new_end.get() - new.source_range.start().get());
                if old_range.is_empty() && new_range.is_empty() {
                    continue;
                }
                edits.push(diff_transform_sync::InputEdit::new(old_range, new_range));
            }
        }
        diff_transform_sync::merge_input_edits(edits)
    }

    /// 结构变更（excerpt 增删、diff 展开折叠）必须在文本事务之外进行。
    ///
    /// 事务期间改变拓扑会让组合事务身份与坐标基准错配；
    /// 这里显式失败，而不是让 end_transaction 在已变化的拓扑上静默收尾。
    pub(crate) fn assert_no_active_transaction(&self, entry: &'static str) {
        assert!(
            self.state.active_transaction.is_none(),
            "{entry} 必须在文本事务之外调用"
        );
    }

    pub(crate) fn subscribe_source(
        source: Entity<LanguageBuffer>,
        track_text: bool,
        cx: &mut Context<Self>,
    ) -> SourceSubscription {
        let source_id = source.entity_id();
        let event = cx.subscribe(&source, move |this, _, event, cx| match event {
            LanguageBufferEvent::TextChanged if track_text => {
                this.state
                    .pending_source_syncs
                    .entry(source_id)
                    .or_default();
                cx.emit(MultiBufferEvent::TextChanged);
                cx.notify();
            }
            LanguageBufferEvent::TextChanged => {}
            LanguageBufferEvent::Reparsed => {
                this.state
                    .pending_source_syncs
                    .entry(source_id)
                    .or_default()
                    .refresh_metadata = true;
                cx.emit(MultiBufferEvent::Reparsed(source_id));
                cx.notify();
            }
            LanguageBufferEvent::MetadataChanged => {
                this.state
                    .pending_source_syncs
                    .entry(source_id)
                    .or_default()
                    .refresh_metadata = true;
                cx.emit(MultiBufferEvent::MetadataChanged);
                cx.notify();
            }
            LanguageBufferEvent::ReloadNeeded => {}
        });
        let text = track_text.then(|| source.read(cx).subscribe());
        SourceSubscription {
            source,
            text,
            _event: event,
        }
    }
}
