//! `MultiBuffer` 的构造入口：空缓冲、单 excerpt、只读与能力变体。
//!
//! 本文件的类型与字段仍定义在 crate 根模块 `multi_buffer.rs`。

use super::*;

impl MultiBuffer {
    /// 统一选择文本历史的所有者。
    ///
    /// 同一个文档模型根据源数量选择历史协调者：
    /// 单源文档复用共享源历史，多源文档由自身协调多个源的历史。
    pub(crate) fn history_owner(&self) -> HistoryOwner {
        if self.singleton_source.is_some() {
            HistoryOwner::SourceBuffer
        } else {
            HistoryOwner::MultiBuffer
        }
    }

    /// 创建空的可编辑组合文档；后续 excerpts 按稳定路径键组织。
    pub fn empty(cx: &mut Context<Self>) -> Self {
        Self::empty_with_capability(Capability::ReadWrite, PathOrder::Natural, cx)
    }

    /// 创建按树形顺序排列文件的组合文档。
    pub fn empty_tree_ordered(cx: &mut Context<Self>) -> Self {
        Self::empty_with_capability(Capability::ReadWrite, PathOrder::Tree, cx)
    }

    /// 创建按树形顺序排列文件的只读组合文档。
    pub fn empty_tree_ordered_read_only(cx: &mut Context<Self>) -> Self {
        Self::empty_with_capability(Capability::ReadOnly, PathOrder::Tree, cx)
    }

    /// 从工作区源构建独立的组合文档（整文件可编辑 excerpt）。
    ///
    /// 普通编辑器的文档统一经此构造：项目共享 LanguageBuffer 只作为工作区源（source），展开 diff hunk 时的 set_excerpts_for_path 只影响本组合文档，不污染项目共享文档。
    /// 单文件文档没有多文件边界，快照据此标记 singleton，显示层不绘制 header/divider。
    pub fn singleton(source: Entity<LanguageBuffer>, cx: &mut Context<Self>) -> Self {
        let line_count = source.read(cx).text_snapshot().line_count();
        let mut multi_buffer = Self::empty(cx);
        multi_buffer.singleton_source = Some(source.clone());
        multi_buffer.set_excerpts_for_path(
            vec![ExcerptRange::line_range(source.clone(), 0..line_count, cx)],
            cx,
        );
        multi_buffer
    }

    fn empty_with_capability(
        capability: Capability,
        path_order: PathOrder,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            state: Self::empty_excerpt_state(cx),
            capability,
            path_order,
            singleton_source: None,
            diffs: Vec::new(),
            diff: None,
            diff_expanded_by_default: false,
            snapshot: MultiBufferSnapshot {
                path_order,
                ..MultiBufferSnapshot::empty()
            },
            snapshot_refresh: SnapshotRefresh::SourceTopologyChanged,
            projection_sync: None,
        }
    }

    /// 空组合状态只保存 source 与其投影映射；组合文本不拥有第二份 Buffer。
    fn empty_excerpt_state(_cx: &mut Context<Self>) -> ExcerptState {
        ExcerptState {
            source_subscriptions: Vec::new(),
            diff_transforms: SumTree::new(()),
            excerpts: SumTree::new(()),
            sources: Vec::new(),
            source_indices: HashMap::new(),
            path_keys: Vec::new(),
            path_key_indices: HashMap::new(),
            capture_names: Arc::from([]),
            projection_version: BufferVersion::INITIAL,
            topology_version: 0,
            metadata_epoch: 0,
            pending_source_syncs: HashMap::new(),
            projection_changes: ProjectionChangeTopic::default(),
            next_transaction_id: TransactionId::INITIAL,
            active_transaction: None,
            active_source_transactions: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }
}
