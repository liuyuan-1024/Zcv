//! Composition 工作流：把 IME start/update/commit/cancel 映射为 Buffer 文本、selection 与 history 变化。
//!
//! 本文件是组合输入唯一的状态机入口；底层坐标换算和相对选区校验分别委托给 `state` 与 `validation`。

use crate::{
    ByteOffset, CompositionState, EngineResult, SelectionSet, TextRange,
    transaction::{ChangeSet, Delta, TransactionMetadata, TransactionSource},
};

use crate::buffer::{Buffer, history::HistoryEntry};

use super::{
    state::{
        absolute_composition_selection, composition_range_after_preedit, resolve_relative_selection,
    },
    validation::validate_composition_relative_selection,
};

impl Buffer {
    /// 返回当前 IME 组合输入状态。
    pub fn composition(&self) -> Option<&CompositionState> {
        self.composition.as_ref()
    }

    pub fn is_composing(&self) -> bool {
        self.composition.is_some()
    }

    /// 开始 IME 组合输入。
    ///
    /// 多光标 / 多选区下采用保守降级策略：只保留 primary selection 作为组合输入目标，
    /// 避免一个系统 IME composition 同时驱动多个插入点。
    pub fn start_composition(&mut self) -> EngineResult<CompositionState> {
        self.ensure_writable()?;

        if let Some(composition) = self.composition.clone() {
            return Ok(composition);
        }

        let original_selection = self.selection.clone();
        self.validate_selection_set(&original_selection)?;

        let primary = *original_selection.primary();
        let range = primary.range();
        let state = CompositionState::new(
            self.text().into_owned(),
            original_selection,
            self.is_dirty(),
            range,
        );

        // IME composition 只跟随 primary selection。这里直接同步 Buffer selection，
        // 让 UI 能观察到多光标降级后的真实编辑目标。
        self.selection = SelectionSet::new(vec![primary]);
        self.composition = Some(state.clone());

        Ok(state)
    }

    /// 更新预编辑文本。
    ///
    /// update 会把 preedit 文本写入 Buffer 以便 UI 读取统一文本流，但事务不进入
    /// Undo 历史。commit 时会从 composition start 前的原始文本到最终提交文本生成
    /// 一个合理的单步 Undo 历史。
    pub fn update_composition(
        &mut self,
        preedit_text: &str,
        selection: Option<crate::CompositionSelection>,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        if self.composition.is_none() {
            self.start_composition()?;
        }

        let state = self
            .composition
            .as_ref()
            .expect("composition must exist after start_composition")
            .clone();

        self.validate_range(state.range)?;

        // preedit_text 与 selection 都是 byte 偏移
        let preedit_len = preedit_text.len();
        let relative_selection = resolve_relative_selection(selection, preedit_len);
        validate_composition_relative_selection(preedit_text, relative_selection)?;

        let range_start = state.range.start();
        let absolute_selection = absolute_composition_selection(range_start, relative_selection)?;
        let after_selection = SelectionSet::new(vec![absolute_selection]);

        let result = self.replace_single_range_with_metadata(
            state.range,
            preedit_text,
            after_selection,
            TransactionMetadata::new(TransactionSource::Composition)
                .without_history()
                .with_description("composition update"),
        )?;

        let mut state = self
            .composition
            .take()
            .expect("composition must still exist while update_composition runs");
        state.range = composition_range_after_preedit(range_start, preedit_len)?;
        state.preedit_text = preedit_text.to_string();
        state.selection = absolute_selection;
        self.composition = Some(state);

        Ok(result)
    }

    /// 提交当前组合输入。
    ///
    /// 如果不存在 active composition，则退化为一次普通的 composition 来源插入 / 替换。
    pub fn commit_composition(
        &mut self,
        commit_text: &str,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        let Some(state) = self.composition.take() else {
            let selections = self.selection.clone();
            return self.replace_selection_ranges_with_metadata(
                selections,
                commit_text,
                TransactionMetadata::new(TransactionSource::Composition)
                    .with_description("composition commit"),
            );
        };

        self.validate_range(state.range)?;

        let range_start = state.range.start();
        let final_head = ByteOffset::new(range_start.get() + commit_text.len());
        let after_selection = SelectionSet::caret(final_head);

        let result = self.replace_single_range_with_metadata(
            state.range,
            commit_text,
            after_selection.clone(),
            TransactionMetadata::new(TransactionSource::Composition)
                .without_history()
                .with_description("composition commit text"),
        )?;

        let after_text = self.text().into_owned();

        if after_text == state.original_text {
            self.set_selection(after_selection)?;
            if !state.original_was_dirty {
                self.mark_clean_internal();
            }
            return Ok(result);
        }

        let entry = HistoryEntry::from_snapshots(
            state.original_text,
            after_text,
            state.original_selection,
            after_selection,
            Some("composition commit".to_string()),
        )?;
        let metadata = TransactionMetadata::new(TransactionSource::Composition)
            .with_description("composition commit");
        self.push_history(entry, &metadata)?;

        Ok(result)
    }

    /// 取消当前组合输入，恢复到 composition start 前的文本和选区。
    pub fn cancel_composition(&mut self) -> EngineResult<Option<(Delta, ChangeSet)>> {
        let Some(state) = self.composition.take() else {
            return Ok(None);
        };

        let full_range = TextRange::new(ByteOffset::ZERO, self.len_bytes())?;
        let after_selection = state.original_selection.clone();

        let result = self.replace_single_range_with_metadata(
            full_range,
            &state.original_text,
            after_selection,
            TransactionMetadata::new(TransactionSource::Composition)
                .without_history()
                .with_description("composition cancel"),
        )?;

        if !state.original_was_dirty {
            self.mark_clean_internal();
        }

        Ok(result)
    }

    pub(in crate::buffer) fn cancel_composition_before_text_edit(&mut self) -> EngineResult<()> {
        if self.composition.is_some() {
            self.cancel_composition()?;
        }

        Ok(())
    }
}
