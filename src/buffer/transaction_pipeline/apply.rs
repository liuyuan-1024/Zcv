//! 事务应用管线：从 Transaction 校验、准备、提交到 selection 映射和 history 收尾的一站式执行路径。
//!
//! 本文件守住失败原子性和版本推进边界；EditList 归一化、存储实现和 public 便利编辑入口不在这里定义。

use crate::{
    BufferVersion, EngineError, EngineResult, LargeTransactionPolicy, PositionMap, SelectionSet,
    errors::{EditError, TransactionError},
    storage::TextStorage,
    transaction::{
        ChangeSet, Delta, DeltaEvent, EditList, Transaction, TransactionRecord, TransactionSource,
    },
};

use crate::buffer::{Buffer, history::HistoryEntry};

use super::prepared::PreparedTransaction;

/// 单个 `EditList` 内所有 `Edit::replacement` 的 UTF-8 字节和。
///
/// 度量口径与 `HistoryEntry::byte_size` 对齐，避免事务 prepare 阶段的预算检查
/// 与最终 `HistoryEntry` 的字节统计漂移。
pub(in crate::buffer) fn edit_list_replacement_bytes(edits: &EditList) -> usize {
    edits
        .as_slice()
        .iter()
        .map(|edit| edit.replacement().len())
        .sum()
}

impl Buffer {
    /// 提交并应用事务。
    ///
    /// 成功将返回增量 Delta 和事务变更集合 ChangeSet，并记录 Undo 历史。
    pub fn apply_transaction(&mut self, tx: Transaction) -> EngineResult<(Delta, ChangeSet)> {
        let (_, delta, changeset) = self.apply_transaction_inner(tx)?;
        Ok((delta, changeset))
    }

    /// 提交并应用事务，返回完整的可回放事实 `TransactionRecord`。
    pub fn apply_transaction_recorded(
        &mut self,
        tx: Transaction,
    ) -> EngineResult<TransactionRecord> {
        let (record, _, _) = self.apply_transaction_inner(tx)?;
        Ok(record)
    }

    /// 在当前 Buffer 上回放一条 `TransactionRecord`，必须 `record.old_version == self.version`。
    ///
    /// 回放走标准 `apply_transaction` 管线，不绕过任何边界校验；返回新生成的
    /// `TransactionRecord`（`transaction_id` 由当前 Buffer 重新分配）。
    pub fn replay_transaction_record(
        &mut self,
        record: &TransactionRecord,
    ) -> EngineResult<TransactionRecord> {
        if record.old_version() != self.version {
            return Err(TransactionError::VersionMismatch {
                expected: self.version,
                actual: record.old_version(),
            }
            .into());
        }

        self.apply_transaction_recorded(record.to_transaction())
    }

    fn apply_transaction_inner(
        &mut self,
        tx: Transaction,
    ) -> EngineResult<(TransactionRecord, Delta, ChangeSet)> {
        self.ensure_writable()?;
        let mut prepared = self.prepare_transaction(tx)?;
        self.apply_large_transaction_policy(&mut prepared)?;

        // Phase 4 引入 Arc<[T]>：以下所有 clone 都是 O(1) 引用计数递增，无堆分配。
        // 仍然显式列出便于读者理解所有权流动；编译器不会自动 elide 这些 Arc::clone。
        let (delta, changeset) = self.apply_edit_list(
            prepared.base_version,
            prepared.edits.clone(),
            prepared.metadata.source(),
        )?;

        let position_map = changeset.position_map();
        let after_selection = self.resolve_after_selection(
            &prepared.before_selection,
            prepared.explicit_after_selection.as_ref(),
            &position_map,
        );

        // 取得事务 id（一定存在，由 apply_edit_list 写入）。
        // 用 EngineBug 而不是 expect 保证不向外 panic。
        let transaction_id = self
            .last_delta_event()
            .map(|event| event.transaction_id())
            .ok_or_else(|| EngineError::EngineBug {
                location: "apply_transaction_inner",
                detail: "apply_edit_list succeeded but no DeltaEvent was emitted".to_string(),
            })?;

        // 构造 TransactionRecord：所有字段都是 Arc-backed 或 Copy，clone 是 O(1)。
        let record = TransactionRecord::new(
            transaction_id,
            delta.old_version(),
            delta.new_version(),
            prepared.edits.clone(),
            prepared.undo_edits.clone(),
            prepared.before_selection.clone(),
            after_selection.clone(),
            prepared.metadata.clone(),
        );

        // 推进 Buffer 选区状态；after_selection 在这里 move 进 Buffer。
        self.selection = after_selection.clone();
        self.finish_transaction(prepared, after_selection)?;

        Ok((record, delta, changeset))
    }

    fn prepare_transaction(&mut self, tx: Transaction) -> EngineResult<PreparedTransaction> {
        self.cancel_composition_for_transaction(tx.metadata().source())?;

        let (base_version, edits, metadata, before_selection, explicit_after_selection) =
            tx.into_parts();

        self.verify_transaction_base_version(base_version)?;
        self.validate_edit_list(&edits)?;

        let before_selection = before_selection.unwrap_or_else(|| self.selection.clone());
        let undo_edits = self.build_inverse_edit_list(&edits)?;
        let redo_edits = edits.clone();

        Ok(PreparedTransaction {
            base_version,
            edits,
            metadata,
            before_selection,
            explicit_after_selection,
            undo_edits,
            redo_edits,
        })
    }

    fn cancel_composition_for_transaction(
        &mut self,
        source: TransactionSource,
    ) -> EngineResult<()> {
        if source != TransactionSource::Composition {
            self.cancel_composition_before_text_edit()?;
        }

        Ok(())
    }

    fn verify_transaction_base_version(&self, base_version: BufferVersion) -> EngineResult<()> {
        if base_version != self.version {
            return Err(crate::TransactionError::VersionMismatch {
                expected: self.version,
                actual: base_version,
            }
            .into());
        }

        Ok(())
    }

    fn resolve_after_selection(
        &self,
        before_selection: &SelectionSet,
        explicit_after_selection: Option<&SelectionSet>,
        position_map: &PositionMap,
    ) -> SelectionSet {
        explicit_after_selection
            .cloned()
            .unwrap_or_else(|| before_selection.map_through_position_map(position_map))
    }

    fn finish_transaction(
        &mut self,
        prepared: PreparedTransaction,
        after_selection: SelectionSet,
    ) -> EngineResult<()> {
        if prepared.metadata.record_history() {
            // Arc::clone：description 字符串只在历史节点持有一份共享
            let description = prepared.metadata.description_arc().cloned();
            let entry = HistoryEntry::new(
                prepared.undo_edits,
                prepared.redo_edits,
                prepared.before_selection,
                after_selection,
                description,
            );
            self.push_history(entry, &prepared.metadata)?;
            return Ok(());
        }

        // record_history=false 提交后，当前节点下的 redo 分支已经基于过期文本，
        // 整体丢弃以避免后续 redo 走到不一致状态；undo 路径保持不变。
        self.drop_unrecorded_redo_branches();
        Ok(())
    }

    /// 在 prepare 之后、commit 之前，按 `LargeFilePolicy` 处理超大事务。
    ///
    /// `Reject`：原子拒绝事务，文本 / 版本 / 历史完全不变。
    /// `SkipHistory`：把 metadata 的 `record_history` 关掉，复用既有
    /// `finish_transaction` 中 `record_history=false` 路径，文本前进但不入历史
    /// 且丢弃当前节点子树。
    fn apply_large_transaction_policy(
        &self,
        prepared: &mut PreparedTransaction,
    ) -> EngineResult<()> {
        let threshold = self.config.large_file.large_transaction_threshold_bytes;
        if threshold == 0 {
            return Ok(());
        }

        let entry_bytes = edit_list_replacement_bytes(&prepared.edits)
            + edit_list_replacement_bytes(&prepared.undo_edits);
        if entry_bytes <= threshold {
            return Ok(());
        }

        match self.config.large_file.large_transaction_policy {
            LargeTransactionPolicy::Reject => Err(EditError::PayloadTooLarge {
                size: entry_bytes,
                limit: threshold,
            }
            .into()),
            LargeTransactionPolicy::SkipHistory => {
                prepared.metadata = prepared.metadata.clone().without_history();
                Ok(())
            }
        }
    }

    /// 把已校验的 `EditList` 落地到 Buffer。
    ///
    /// **半提交修复**：在文本变异**之前**完成所有可失败步骤（version 检查、validate、
    /// id 预约、`version.next()` 算溢出）。从文本变异起的每一步都必须是不可失败的；
    /// `storage.replace` 在 prepare 校验后失败属于引擎不变量违反，作为 `EngineError::EngineBug`
    /// 暴露而不是静默回滚。这消除了"先 swap storage 再 bump_version" 的半提交窗口。
    ///
    /// **去 storage clone**：不再 `self.storage.clone()` —— Phase 1 的 `validate_edit_list`
    /// + 倒序应用已经保证文本变异不会语义性失败；rollback-by-clone 既不必要也不便宜。
    pub(in crate::buffer) fn apply_edit_list(
        &mut self,
        base_version: BufferVersion,
        tx_edits: EditList,
        source: TransactionSource,
    ) -> EngineResult<(Delta, ChangeSet)> {
        // ===== Fallible 段：在任何文本变异前完成全部可失败检查 =====
        self.ensure_writable()?;

        if base_version != self.version {
            return Err(TransactionError::VersionMismatch {
                expected: self.version,
                actual: base_version,
            }
            .into());
        }

        self.validate_edit_list(&tx_edits)?;
        let transaction_id = self.reserve_transaction_id()?;
        let old_version = self.version;
        let new_version = old_version.next().ok_or(EngineError::VersionOverflow)?;

        // ===== Infallible 段：从这里起文本变异不允许失败 =====
        // 倒序应用：所有 edit 都是基于旧文本坐标的；后向前应用保证前面的 edit 偏移不漂移。
        // EditList::new 保证 edits 互不重叠并已按 start 升序排列。
        for edit in tx_edits.as_slice().iter().rev() {
            self.storage
                .replace(edit.range(), edit.replacement())
                .map_err(|err| EngineError::EngineBug {
                    location: "apply_edit_list",
                    detail: format!("validated edit failed at range {:?}: {err:?}", edit.range()),
                })?;
        }

        // 文本变异已完成且必然成功；现在原子推进 version。
        self.version = new_version;

        let changeset = ChangeSet::from_edit_list(&tx_edits);
        let position_map = changeset.position_map();

        let delta = Delta::new(old_version, new_version, tx_edits);

        // Arc-backed clone：O(1) 引用计数递增
        self.push_delta_event(DeltaEvent::new(
            transaction_id,
            source,
            delta.clone(),
            changeset.clone(),
            position_map,
        ));

        Ok((delta, changeset))
    }
}
