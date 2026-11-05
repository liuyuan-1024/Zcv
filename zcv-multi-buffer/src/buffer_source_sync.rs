//! `MultiBuffer` 的源文档变化同步与源快照安装。
//!
//! 本文件的类型与字段仍定义在 crate 根模块 `multi_buffer.rs`。

use super::*;

impl MultiBuffer {
    /// 从 MultiBuffer 自己拥有的源订阅拉取下一段连续变化并推进投影。
    ///
    /// `LanguageBufferEvent` 只是唤醒信号；
    /// 直接编辑、外部编辑和历史回放都经过此入口，因而同一源只有一个增量游标，不会重放已消费的旧事件。
    pub(crate) fn synchronize_source_change(
        &mut self,
        source_id: EntityId,
        expanded_excerpts: Option<&HashSet<usize>>,
        refresh_metadata: bool,
        cx: &mut Context<Self>,
    ) -> Option<TextChangeBatch> {
        let tracks_text = self
            .state
            .source_subscriptions
            .iter()
            .find(|state| state.source.entity_id() == source_id)
            .map(|state| state.text.is_some())?;
        if !tracks_text {
            if refresh_metadata {
                self.refresh_source_metadata_if_current(source_id, cx);
            }
            return None;
        }
        let source_change = self
            .state
            .source_subscriptions
            .iter()
            .find(|state| state.source.entity_id() == source_id)
            .and_then(|state| state.text.as_ref())
            .map(TextSubscription::consume)?;
        if source_change.is_empty() {
            // 文本变化已被主动同步；延迟到达的事件只需刷新语法快照。
            self.refresh_source_snapshot(source_id, cx);
            return None;
        }
        let stored_version = self
            .state
            .sources
            .iter()
            .find(|source| source.entity.entity_id() == source_id)
            .map(|source| source.text.version())?;
        assert_eq!(
            source_change.old_version(),
            Some(stored_version),
            "MultiBuffer 必须按自己的源订阅连续消费文本变化"
        );
        let position_map = source_change.position_map();
        // 普通编辑只更新受影响 source 的派生坐标；BufferDiff 仍独立维护 hunk 拓扑。
        self.apply_source_change(
            source_id,
            &position_map,
            &source_change,
            expanded_excerpts,
            cx,
        );
        if refresh_metadata {
            self.refresh_source_snapshot(source_id, cx);
        }
        Some(source_change)
    }

    fn refresh_source_snapshot(&mut self, source_id: EntityId, cx: &App) {
        let Some(source) = self
            .state
            .sources
            .iter()
            .find(|source| source.entity.entity_id() == source_id)
            .map(|source| source.entity.clone())
        else {
            return;
        };
        let snapshot = source.read(cx).snapshot();
        self.install_source_snapshot(source_id, snapshot);
    }

    /// Deleted 源的文本由 BufferDiff 推进；其语法只能同步到当前投影已采用的同一文本版本。
    fn refresh_source_metadata_if_current(&mut self, source_id: EntityId, cx: &App) {
        let Some((source, text_version)) = self
            .state
            .sources
            .iter()
            .find(|source| source.entity.entity_id() == source_id)
            .map(|source| (source.entity.clone(), source.text.version()))
        else {
            return;
        };
        let snapshot = source.read(cx).snapshot();
        if snapshot.text.version() != text_version {
            return;
        }
        self.install_source_snapshot(source_id, snapshot);
    }

    pub(crate) fn install_source_snapshot(
        &mut self,
        source_id: EntityId,
        snapshot: LanguageBufferSnapshot,
    ) {
        self.mark_snapshot_changed();
        self.state.metadata_epoch = self.state.metadata_epoch.wrapping_add(1);
        if let Some(excerpt_source) = self
            .state
            .sources
            .iter_mut()
            .find(|source| source.entity.entity_id() == source_id)
        {
            excerpt_source.is_dirty = snapshot.is_dirty;
            excerpt_source.text = snapshot.text.clone();
            excerpt_source.syntax = snapshot.syntax.clone();
            excerpt_source.highlight_cache = Arc::clone(&snapshot.highlight_cache);
            excerpt_source.settings = snapshot_settings(&snapshot);
        }
        self.mark_source_snapshot_changed(source_id);
        let previous_capture_names = Arc::clone(&self.state.capture_names);
        self.state.capture_names = rebuild_capture_table(&mut self.state.sources);
        if self.state.capture_names != previous_capture_names {
            self.mark_all_source_snapshots_changed();
        }
    }

    fn mark_source_snapshot_changed(&mut self, source_id: EntityId) {
        let Some(source_index) = self.state.source_indices.get(&source_id).copied() else {
            return;
        };
        self.mark_snapshot_changed();
        if let SnapshotRefresh::SourceUpdates(updates) = &mut self.snapshot_refresh {
            updates.insert(source_index);
        }
    }

    fn mark_all_source_snapshots_changed(&mut self) {
        self.mark_snapshot_changed();
        if let SnapshotRefresh::SourceUpdates(updates) = &mut self.snapshot_refresh {
            updates.extend(0..self.state.sources.len());
        }
    }

    /// 把源 Buffer 的版本化编辑同步到组合投影。
    fn apply_source_change(
        &mut self,
        source_id: EntityId,
        source_position_map: &PositionMap,
        source_change: &TextChangeBatch,
        expanded_excerpts: Option<&HashSet<usize>>,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self
            .state
            .sources
            .iter()
            .find(|source| source.entity.entity_id() == source_id)
            .map(|source| source.entity.clone())
        else {
            return;
        };
        let before = self.projection_trees();
        let old_text = self.state.sources[self.state.source_indices[&source_id]]
            .text
            .clone();
        let snapshot = source.read(cx).snapshot();
        // capture 表只在源语法捕获集合变化时重建；纯文本编辑保持已有全局索引不变。
        let captures_changed = self
            .state
            .sources
            .iter()
            .find(|source| source.entity.entity_id() == source_id)
            .is_some_and(|source| source.syntax.capture_names() != snapshot.syntax.capture_names());

        if let Some(excerpt_source) = self
            .state
            .sources
            .iter_mut()
            .find(|source| source.entity.entity_id() == source_id)
        {
            excerpt_source.settings = snapshot_settings(&snapshot);
            excerpt_source.is_dirty = snapshot.is_dirty;
            excerpt_source.text = snapshot.text;
            excerpt_source.syntax = snapshot.syntax;
            excerpt_source.highlight_cache = snapshot.highlight_cache;
        }
        self.mark_source_snapshot_changed(source_id);
        if captures_changed {
            self.state.capture_names = rebuild_capture_table(&mut self.state.sources);
            self.mark_all_source_snapshots_changed();
        }
        // 绝对输出坐标由树摘要推导：源范围变化只 splice 受影响路径的 item，其余路径不变。
        let records =
            self.splice_source_path(source_id, source_position_map, expanded_excerpts, cx);
        let edits = self.source_input_edits(source_change, &records);
        let output_edits =
            self.sync_diff_transforms(&before, edits, Some((source_id, &old_text)), cx);
        self.publish_projection_change(SourceIncremental {
            batch: source_change.projected_from(output_edits),
        });
        self.emit_text_changed(cx);
    }
}
