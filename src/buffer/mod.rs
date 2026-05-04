//! 最小可编辑 Buffer。
//!
//! M3.5 目标：
//! - 内部编辑坐标全面迁移到 CharOffset
//! - TextRange 改为 CharOffset 区间
//! - Buffer / Transaction / ChangeSet / History 均使用字符坐标
//! - ByteOffset 不再参与编辑 API，为 M4 接入 ropey 做准备

mod line_index;

use std::sync::Arc;

use crate::{
    BufferConfig, BufferVersion, CharOffset, CoordinateError, EditError, EngineError, EngineResult,
    Line, Position, SelectionSnapshot, TextRange,
    storage::{StringStorage, TextStorage},
    transaction::{
        ChangeSet, Delta, Edit, EditList, Transaction, TransactionMergePolicy, TransactionMetadata,
    },
};

use line_index::LineIndex;

/// 不可变文本快照。
///
/// M3 先基于 `Arc<str>` 验证快照语义。M4 替换为 ropey 后，
/// 可以把内部实现替换为 O(1) 共享 Rope snapshot。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    text: Arc<str>,
    version: BufferVersion,
    line_index: LineIndex,
}

impl Snapshot {
    fn new(text: Arc<str>, version: BufferVersion) -> Self {
        let line_index = LineIndex::build(&text);
        Self {
            text,
            version,
            line_index,
        }
    }

    pub fn text(&self) -> &str {
        self.text.as_ref()
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    pub fn len_chars(&self) -> CharOffset {
        CharOffset::new(self.text.chars().count())
    }

    pub fn line_count(&self) -> usize {
        self.line_index.line_count()
    }

    pub fn line_start(&self, line: Line) -> EngineResult<CharOffset> {
        self.line_index.line_start(line)
    }

    pub fn char_to_position(&self, offset: CharOffset) -> EngineResult<Position> {
        self.line_index.char_to_position(self.text(), offset)
    }

    pub fn position_to_char(&self, position: Position) -> EngineResult<CharOffset> {
        self.line_index.position_to_char(self.text(), position)
    }

    pub fn is_stale_for(&self, buffer: &Buffer) -> bool {
        self.version != buffer.version()
    }
}

/// M3 历史状态摘要。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryStatus {
    pub undo_depth: usize,
    pub redo_depth: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HistoryEntry {
    before_text: String,
    after_text: String,
    undo_edits: EditList,
    redo_edits: EditList,
    before_selection: Option<SelectionSnapshot>,
    after_selection: Option<SelectionSnapshot>,
    description: Option<String>,
}

impl HistoryEntry {
    fn new(
        before_text: String,
        after_text: String,
        undo_edits: EditList,
        redo_edits: EditList,
        before_selection: Option<SelectionSnapshot>,
        after_selection: Option<SelectionSnapshot>,
        description: Option<String>,
    ) -> Self {
        Self {
            before_text,
            after_text,
            undo_edits,
            redo_edits,
            before_selection,
            after_selection,
            description,
        }
    }

    fn from_snapshots(
        before_text: String,
        after_text: String,
        before_selection: Option<SelectionSnapshot>,
        after_selection: Option<SelectionSnapshot>,
        description: Option<String>,
    ) -> EngineResult<Self> {
        let before_range = TextRange::new(
            CharOffset::ZERO,
            CharOffset::new(before_text.chars().count()),
        )?;
        let after_range = TextRange::new(
            CharOffset::ZERO,
            CharOffset::new(after_text.chars().count()),
        )?;

        let redo_edits = EditList::new(vec![Edit::replace(before_range, after_text.clone())])?;
        let undo_edits = EditList::new(vec![Edit::replace(after_range, before_text.clone())])?;

        Ok(Self::new(
            before_text,
            after_text,
            undo_edits,
            redo_edits,
            before_selection,
            after_selection,
            description,
        ))
    }
}

/// 最小可编辑 Buffer。
#[derive(Debug, Clone)]
pub struct Buffer {
    config: BufferConfig,
    storage: StringStorage,
    version: BufferVersion,
    saved_version: BufferVersion,
    line_index: LineIndex,
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    selection: Option<SelectionSnapshot>,
}

impl Buffer {
    /// 创建空 Buffer。
    pub fn new(config: BufferConfig) -> EngineResult<Self> {
        Self::from_text(String::new(), config)
    }

    /// 从已有文本创建 Buffer。
    pub fn from_text(text: String, config: BufferConfig) -> EngineResult<Self> {
        let line_index = LineIndex::build(&text);

        Ok(Self {
            config,
            storage: StringStorage::new(text),
            version: BufferVersion::INITIAL,
            saved_version: BufferVersion::INITIAL,
            line_index,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            selection: None,
        })
    }

    pub fn config(&self) -> &BufferConfig {
        &self.config
    }

    pub fn text(&self) -> &str {
        self.storage.text()
    }

    pub fn len_chars(&self) -> CharOffset {
        self.storage.len_chars()
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    pub fn saved_version(&self) -> BufferVersion {
        self.saved_version
    }

    pub fn is_dirty(&self) -> bool {
        self.version != self.saved_version
    }

    pub fn mark_saved(&mut self) {
        self.saved_version = self.version;
    }

    pub fn line_count(&self) -> usize {
        self.line_index.line_count()
    }

    pub fn line_start(&self, line: Line) -> EngineResult<CharOffset> {
        self.line_index.line_start(line)
    }

    pub fn char_to_position(&self, offset: CharOffset) -> EngineResult<Position> {
        self.line_index.char_to_position(self.text(), offset)
    }

    pub fn position_to_char(&self, position: Position) -> EngineResult<CharOffset> {
        self.line_index.position_to_char(self.text(), position)
    }

    /// 创建绑定当前版本的不可变快照。
    pub fn snapshot(&self) -> Snapshot {
        Snapshot::new(Arc::from(self.text()), self.version)
    }

    /// 判断给定版本是否已经相对当前 Buffer 过期。
    pub fn is_version_stale(&self, version: BufferVersion) -> bool {
        version != self.version
    }

    pub fn is_snapshot_stale(&self, snapshot: &Snapshot) -> bool {
        snapshot.version() != self.version
    }

    /// M3 只提供用于历史恢复的轻量 selection 状态。
    /// 完整 Selection / Multi Cursor 模型留到后续阶段。
    pub fn selection_snapshot(&self) -> Option<&SelectionSnapshot> {
        self.selection.as_ref()
    }

    pub fn set_selection_snapshot(&mut self, selection: Option<SelectionSnapshot>) {
        self.selection = selection;
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn history_status(&self) -> HistoryStatus {
        HistoryStatus {
            undo_depth: self.undo_stack.len(),
            redo_depth: self.redo_stack.len(),
        }
    }

    /// 提交并应用事务。
    ///
    /// 成功将返回增量事件 Delta 和位置映射器 ChangeSet，并记录 Undo 历史。
    pub fn apply_transaction(&mut self, tx: Transaction) -> EngineResult<(Delta, ChangeSet)> {
        let (base_version, tx_edits, metadata, tx_before_selection, tx_after_selection) =
            tx.into_parts();

        if base_version != self.version {
            return Err(crate::TransactionError::VersionMismatch {
                expected: self.version,
                actual: base_version,
            }
            .into());
        }

        self.validate_edit_list(&tx_edits)?;

        let before_text = self.text().to_string();
        let before_selection = tx_before_selection.or_else(|| self.selection.clone());
        let undo_edits = Self::build_inverse_edit_list(&before_text, &tx_edits)?;
        let redo_edits = tx_edits.clone();

        let (delta, changeset) = self.apply_edit_list(base_version, tx_edits)?;

        if let Some(selection) = tx_after_selection {
            self.selection = Some(selection);
        }

        let after_selection = self.selection.clone();
        let after_text = self.text().to_string();

        if metadata.record_history {
            let entry = HistoryEntry::new(
                before_text,
                after_text,
                undo_edits,
                redo_edits,
                before_selection,
                after_selection,
                metadata.description.clone(),
            );

            self.push_history(entry, &metadata)?;
        } else {
            // 任何新的文本变异都会让已有 redo 分支失效；Undo / Redo 自身走
            // apply_edit_list，不会触发这里。
            self.redo_stack.clear();
        }

        Ok((delta, changeset))
    }

    /// 撤销最近一次历史节点。
    ///
    /// 没有可撤销历史时返回 `Ok(None)`，避免把空历史当作错误。
    pub fn undo(&mut self) -> EngineResult<Option<(Delta, ChangeSet)>> {
        let Some(entry) = self.undo_stack.pop() else {
            return Ok(None);
        };

        let tx_edits = entry.undo_edits.clone();
        let result = self.apply_edit_list(self.version, tx_edits)?;
        self.selection = entry.before_selection.clone();
        self.redo_stack.push(entry);

        Ok(Some(result))
    }

    /// 重做最近一次被撤销的历史节点。
    ///
    /// 没有可重做历史时返回 `Ok(None)`。
    pub fn redo(&mut self) -> EngineResult<Option<(Delta, ChangeSet)>> {
        let Some(entry) = self.redo_stack.pop() else {
            return Ok(None);
        };

        let tx_edits = entry.redo_edits.clone();
        let result = self.apply_edit_list(self.version, tx_edits)?;
        self.selection = entry.after_selection.clone();
        self.undo_stack.push(entry);

        Ok(Some(result))
    }

    pub fn insert(&mut self, offset: CharOffset, text: &str) -> EngineResult<()> {
        let range = TextRange::new(offset, offset)?;
        self.replace(range, text)
    }

    pub fn delete(&mut self, range: TextRange) -> EngineResult<()> {
        self.replace(range, "")
    }

    /// 替换指定字符范围的文本，支持插入和删除。
    ///
    /// M3 起该便利 API 也会走 Transaction，从而进入 Undo 历史。
    pub fn replace(&mut self, range: TextRange, replacement: &str) -> EngineResult<()> {
        self.validate_range(range)?;
        self.validate_edit_boundary(range.start())?;
        self.validate_edit_boundary(range.end())?;

        // no-op 不递增版本，也不污染 dirty / history。
        if self.slice_text(range)? == replacement {
            return Ok(());
        }

        let tx = Transaction::from_edits(
            self.version,
            vec![Edit::replace(range, replacement.to_string())],
        )?;

        self.apply_transaction(tx)?;
        Ok(())
    }

    fn push_history(
        &mut self,
        entry: HistoryEntry,
        metadata: &TransactionMetadata,
    ) -> EngineResult<()> {
        if metadata.merge_policy == TransactionMergePolicy::MergeWithPrevious
            && self.redo_stack.is_empty()
        {
            if let Some(previous) = self.undo_stack.pop() {
                let description = entry.description.clone().or(previous.description.clone());
                let merged = HistoryEntry::from_snapshots(
                    previous.before_text,
                    entry.after_text,
                    previous.before_selection,
                    entry.after_selection,
                    description,
                )?;
                self.undo_stack.push(merged);
                self.truncate_undo_history_to_budget();
                return Ok(());
            }
        }

        self.undo_stack.push(entry);
        self.redo_stack.clear();
        self.truncate_undo_history_to_budget();

        Ok(())
    }

    fn truncate_undo_history_to_budget(&mut self) {
        let max = self.config.large_file.max_undo_history;
        if max == 0 {
            self.undo_stack.clear();
            return;
        }

        while self.undo_stack.len() > max {
            self.undo_stack.remove(0);
        }
    }

    fn build_inverse_edit_list(old_text: &str, edits: &EditList) -> EngineResult<EditList> {
        let mut inverse = Vec::with_capacity(edits.len());
        let mut diff = 0isize;

        for edit in edits.as_slice() {
            let old_start = edit.range.start().get();
            let old_end = edit.range.end().get();
            let deleted_text = slice_chars(old_text, edit.range)?.to_string();

            let new_start = (old_start as isize + diff).max(0) as usize;
            let new_end = new_start + edit.replacement.chars().count();
            let new_range = TextRange::new(CharOffset::new(new_start), CharOffset::new(new_end))?;

            inverse.push(Edit::replace(new_range, deleted_text));

            diff += edit.replacement.chars().count() as isize - (old_end - old_start) as isize;
        }

        Ok(EditList::new(inverse)?)
    }

    fn apply_edit_list(
        &mut self,
        base_version: BufferVersion,
        tx_edits: EditList,
    ) -> EngineResult<(Delta, ChangeSet)> {
        if base_version != self.version {
            return Err(crate::TransactionError::VersionMismatch {
                expected: self.version,
                actual: base_version,
            }
            .into());
        }

        // 1. 预检查：所有 edit 必须在当前旧文本字符坐标系中合法。
        self.validate_edit_list(&tx_edits)?;

        let edits = tx_edits.as_slice().to_vec();
        let old_version = self.version;

        // 2. 在 clone 上应用，确保未来 storage.replace 失败时不污染当前 Buffer。
        let mut new_storage = self.storage.clone();

        let mut reverse_edits = edits;
        reverse_edits.reverse();

        for edit in reverse_edits {
            new_storage.replace(edit.range, &edit.replacement)?;
        }

        // 3. 全部成功后再一次性提交 storage / line_index / version。
        let new_line_index = LineIndex::build(new_storage.text());

        self.storage = new_storage;
        self.line_index = new_line_index;
        self.bump_version()?;

        let new_version = self.version;

        let changeset = ChangeSet::from_edit_list(&tx_edits);

        let delta = Delta {
            old_version,
            new_version,
            edits: tx_edits,
        };

        Ok((delta, changeset))
    }

    fn validate_edit_list(&self, edits: &EditList) -> EngineResult<()> {
        for edit in edits.as_slice() {
            self.validate_range(edit.range)?;
            self.validate_edit_boundary(edit.range.start())?;
            self.validate_edit_boundary(edit.range.end())?;
        }

        Ok(())
    }

    /// 校验范围是否合法，超出文本字符长度返回错误。
    fn validate_range(&self, range: TextRange) -> EngineResult<()> {
        if range.end().get() > self.text().chars().count() {
            return Err(EditError::RangeOutOfBounds { range }.into());
        }

        Ok(())
    }

    /// 校验编辑边界是否合法，超出文本范围或落在 CRLF 中间返回错误。
    fn validate_edit_boundary(&self, offset: CharOffset) -> EngineResult<()> {
        let value = offset.get();
        let text = self.text();
        let len_chars = text.chars().count();

        if value > len_chars {
            return Err(CoordinateError::OutOfBounds(offset).into());
        }

        if is_crlf_middle(text, value) {
            return Err(EditError::InvalidBoundary { offset }.into());
        }

        Ok(())
    }

    fn slice_text(&self, range: TextRange) -> EngineResult<&str> {
        slice_chars(self.text(), range)
    }

    /// 递增版本号，溢出时返回错误。
    fn bump_version(&mut self) -> EngineResult<()> {
        self.version = self.version.next().ok_or(EngineError::VersionOverflow)?;
        Ok(())
    }
}

fn slice_chars(text: &str, range: TextRange) -> EngineResult<&str> {
    let start = char_to_byte_index(text, range.start())?;
    let end = char_to_byte_index(text, range.end())?;

    Ok(&text[start..end])
}

fn char_to_byte_index(text: &str, offset: CharOffset) -> EngineResult<usize> {
    let char_offset = offset.get();
    let len_chars = text.chars().count();

    if char_offset > len_chars {
        return Err(CoordinateError::OutOfBounds(offset).into());
    }

    if char_offset == len_chars {
        return Ok(text.len());
    }

    text.char_indices()
        .nth(char_offset)
        .map(|(byte_idx, _)| byte_idx)
        .ok_or_else(|| CoordinateError::OutOfBounds(offset).into())
}

fn is_crlf_middle(text: &str, offset: usize) -> bool {
    offset > 0
        && offset < text.chars().count()
        && char_at(text, offset - 1) == Some('\r')
        && char_at(text, offset) == Some('\n')
}

fn char_at(text: &str, char_offset: usize) -> Option<char> {
    text.chars().nth(char_offset)
}
