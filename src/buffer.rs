//! 最小可编辑 Buffer。
//!
//! M4 目标：
//! - 默认文本后端切换为 RopeyStorage
//! - Buffer / Transaction / ChangeSet / History 继续使用 CharOffset
//! - Snapshot 基于 ropey clone，避免每次快照复制全文
//!
//! M5A 目标：
//! - 暴露 IDE 必需坐标转换：ByteOffset / CharOffset / UTF-16 Position
//! - 暴露 grapheme cluster 边界查询与安全光标移动辅助
//! - 暴露文件换行风格识别
//!
//! M5B 目标：
//! - 暴露 Tab Stop / DisplayWidthPolicy 驱动的视觉列数学
//! - 支持 logical column -> display column
//! - 支持 display column -> 最近合法 logical column / CharOffset
//!
//! M6 目标：
//! - Buffer 直接持有 SelectionSet，不再以 SelectionSnapshot 作为主模型
//! - Transaction / History 直接恢复 SelectionSet
//! - 支持多光标插入、删除、替换
//! - M6B 支持 Word / Identifier / Subword / Symbol 移动语义

use std::borrow::Cow;

use unicode_segmentation::UnicodeSegmentation;

use crate::{
    BufferConfig, BufferVersion, ByteOffset, CharOffset, CoordinateError, DisplayColumn,
    DisplayColumnAffinity, EditError, EngineError, EngineResult, Line, LineEndingStyle,
    LogicalColumn, MovementDirection, MovementUnit, Position, Selection, SelectionSet, TextRange,
    Utf16Position, WordBoundaryPolicy,
    storage::{RopeySnapshot, RopeyStorage, TextRead, TextStorage},
    transaction::{
        ChangeSet, Delta, Edit, EditList, Transaction, TransactionMergePolicy, TransactionMetadata,
        TransactionSource,
    },
};

/// 不可变文本快照。
#[derive(Debug, Clone)]
pub struct Snapshot {
    storage: RopeySnapshot,
    version: BufferVersion,
    config: BufferConfig,
}

impl Snapshot {
    fn new(storage: RopeySnapshot, version: BufferVersion, config: BufferConfig) -> Self {
        Self {
            storage,
            version,
            config,
        }
    }

    pub fn text(&self) -> Cow<'_, str> {
        self.storage.text()
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    pub fn config(&self) -> &BufferConfig {
        &self.config
    }

    pub fn len_chars(&self) -> CharOffset {
        self.storage.len_chars()
    }

    pub fn len_bytes(&self) -> usize {
        self.storage.len_bytes()
    }

    pub fn len_utf16_cu(&self) -> usize {
        self.storage.len_utf16_cu()
    }

    pub fn line_count(&self) -> usize {
        self.storage.line_count()
    }

    pub fn line_start(&self, line: Line) -> EngineResult<CharOffset> {
        self.storage.line_start(line)
    }

    pub fn char_to_position(&self, offset: CharOffset) -> EngineResult<Position> {
        self.storage.char_to_position(offset)
    }

    pub fn position_to_char(&self, position: Position) -> EngineResult<CharOffset> {
        self.storage.position_to_char(position)
    }

    pub fn byte_to_char(&self, offset: ByteOffset) -> EngineResult<CharOffset> {
        self.storage.byte_to_char(offset)
    }

    pub fn char_to_byte(&self, offset: CharOffset) -> EngineResult<ByteOffset> {
        self.storage.char_to_byte(offset)
    }

    pub fn char_to_utf16_position(&self, offset: CharOffset) -> EngineResult<Utf16Position> {
        self.storage.char_to_utf16_position(offset)
    }

    pub fn utf16_position_to_char(&self, position: Utf16Position) -> EngineResult<CharOffset> {
        self.storage.utf16_position_to_char(position)
    }

    pub fn byte_to_utf16_position(&self, offset: ByteOffset) -> EngineResult<Utf16Position> {
        self.storage.byte_to_utf16_position(offset)
    }

    pub fn utf16_position_to_byte(&self, position: Utf16Position) -> EngineResult<ByteOffset> {
        self.storage.utf16_position_to_byte(position)
    }

    pub fn is_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<bool> {
        self.storage.is_grapheme_boundary(offset)
    }

    pub fn validate_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<()> {
        if self.storage.is_grapheme_boundary(offset)? {
            Ok(())
        } else {
            Err(CoordinateError::InvalidGraphemeBoundary(offset).into())
        }
    }

    pub fn previous_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.storage.previous_grapheme_boundary(offset)
    }

    pub fn next_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.storage.next_grapheme_boundary(offset)
    }

    pub fn line_ending_style(&self) -> LineEndingStyle {
        self.storage.line_ending_style()
    }

    pub fn next_tab_stop(&self, display_column: DisplayColumn) -> DisplayColumn {
        next_tab_stop(display_column, self.config.tab.tab_width())
    }

    pub fn char_to_display_column(&self, offset: CharOffset) -> EngineResult<DisplayColumn> {
        char_to_display_column_in_text(&self.storage, &self.config, offset)
    }

    pub fn logical_to_display_column(
        &self,
        line: Line,
        column: LogicalColumn,
    ) -> EngineResult<DisplayColumn> {
        logical_to_display_column_in_text(&self.storage, &self.config, line, column)
    }

    pub fn display_to_logical_column(
        &self,
        line: Line,
        column: DisplayColumn,
    ) -> EngineResult<LogicalColumn> {
        display_to_logical_column_in_text(
            &self.storage,
            &self.config,
            line,
            column,
            self.config.display_width.affinity,
        )
    }

    pub fn display_to_logical_column_with_affinity(
        &self,
        line: Line,
        column: DisplayColumn,
        affinity: DisplayColumnAffinity,
    ) -> EngineResult<LogicalColumn> {
        display_to_logical_column_in_text(&self.storage, &self.config, line, column, affinity)
    }

    pub fn display_column_to_char(
        &self,
        line: Line,
        column: DisplayColumn,
    ) -> EngineResult<CharOffset> {
        let logical = self.display_to_logical_column(line, column)?;
        self.storage.position_to_char(Position::new(line, logical))
    }

    pub fn display_column_to_char_with_affinity(
        &self,
        line: Line,
        column: DisplayColumn,
        affinity: DisplayColumnAffinity,
    ) -> EngineResult<CharOffset> {
        let logical = self.display_to_logical_column_with_affinity(line, column, affinity)?;
        self.storage.position_to_char(Position::new(line, logical))
    }

    pub fn is_stale_for(&self, buffer: &Buffer) -> bool {
        self.version != buffer.version()
    }
}

impl PartialEq for Snapshot {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version && self.text() == other.text()
    }
}

impl Eq for Snapshot {}

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
    before_selection: SelectionSet,
    after_selection: SelectionSet,
    description: Option<String>,
}

impl HistoryEntry {
    fn new(
        before_text: String,
        after_text: String,
        undo_edits: EditList,
        redo_edits: EditList,
        before_selection: SelectionSet,
        after_selection: SelectionSet,
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
        before_selection: SelectionSet,
        after_selection: SelectionSet,
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
    storage: RopeyStorage,
    version: BufferVersion,
    saved_version: BufferVersion,
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    selection: SelectionSet,
}

impl Buffer {
    /// 创建空 Buffer。
    pub fn new(config: BufferConfig) -> EngineResult<Self> {
        Self::from_text(String::new(), config)
    }

    /// 从已有文本创建 Buffer。
    pub fn from_text(text: String, config: BufferConfig) -> EngineResult<Self> {
        Ok(Self {
            config,
            storage: RopeyStorage::new(text),
            version: BufferVersion::INITIAL,
            saved_version: BufferVersion::INITIAL,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            selection: SelectionSet::default(),
        })
    }

    pub fn config(&self) -> &BufferConfig {
        &self.config
    }

    /// 返回全文。
    ///
    /// M4 后该方法返回 Cow，而不是 `&str`，避免 public API 继续承诺全文连续内存。
    /// 热路径请优先用 Snapshot / slice / line API。
    pub fn text(&self) -> Cow<'_, str> {
        self.storage.text()
    }

    pub fn len_chars(&self) -> CharOffset {
        self.storage.len_chars()
    }

    pub fn len_bytes(&self) -> usize {
        self.storage.len_bytes()
    }

    pub fn len_utf16_cu(&self) -> usize {
        self.storage.len_utf16_cu()
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
        self.storage.line_count()
    }

    pub fn line_start(&self, line: Line) -> EngineResult<CharOffset> {
        self.storage.line_start(line)
    }

    pub fn char_to_position(&self, offset: CharOffset) -> EngineResult<Position> {
        self.storage.char_to_position(offset)
    }

    pub fn position_to_char(&self, position: Position) -> EngineResult<CharOffset> {
        self.storage.position_to_char(position)
    }

    pub fn byte_to_char(&self, offset: ByteOffset) -> EngineResult<CharOffset> {
        self.storage.byte_to_char(offset)
    }

    pub fn char_to_byte(&self, offset: CharOffset) -> EngineResult<ByteOffset> {
        self.storage.char_to_byte(offset)
    }

    pub fn char_to_utf16_position(&self, offset: CharOffset) -> EngineResult<Utf16Position> {
        self.storage.char_to_utf16_position(offset)
    }

    pub fn utf16_position_to_char(&self, position: Utf16Position) -> EngineResult<CharOffset> {
        self.storage.utf16_position_to_char(position)
    }

    pub fn byte_to_utf16_position(&self, offset: ByteOffset) -> EngineResult<Utf16Position> {
        self.storage.byte_to_utf16_position(offset)
    }

    pub fn utf16_position_to_byte(&self, position: Utf16Position) -> EngineResult<ByteOffset> {
        self.storage.utf16_position_to_byte(position)
    }

    pub fn is_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<bool> {
        self.storage.is_grapheme_boundary(offset)
    }

    pub fn validate_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<()> {
        if self.storage.is_grapheme_boundary(offset)? {
            Ok(())
        } else {
            Err(CoordinateError::InvalidGraphemeBoundary(offset).into())
        }
    }

    pub fn previous_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.storage.previous_grapheme_boundary(offset)
    }

    pub fn next_grapheme_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.storage.next_grapheme_boundary(offset)
    }

    /// M6B：按给定移动粒度寻找前一个边界。
    pub fn previous_movement_boundary(
        &self,
        offset: CharOffset,
        unit: MovementUnit,
    ) -> EngineResult<CharOffset> {
        self.movement_boundary(offset, MovementDirection::Previous, unit)
    }

    /// M6B：按给定移动粒度寻找后一个边界。
    pub fn next_movement_boundary(
        &self,
        offset: CharOffset,
        unit: MovementUnit,
    ) -> EngineResult<CharOffset> {
        self.movement_boundary(offset, MovementDirection::Next, unit)
    }

    /// M6B：统一移动边界入口。
    pub fn movement_boundary(
        &self,
        offset: CharOffset,
        direction: MovementDirection,
        unit: MovementUnit,
    ) -> EngineResult<CharOffset> {
        movement_boundary_in_text(
            &self.storage,
            self.config.word_boundary,
            offset,
            direction,
            unit,
        )
    }

    pub fn previous_word_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.previous_movement_boundary(offset, MovementUnit::Word)
    }

    pub fn next_word_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.next_movement_boundary(offset, MovementUnit::Word)
    }

    pub fn previous_identifier_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.previous_movement_boundary(offset, MovementUnit::Identifier)
    }

    pub fn next_identifier_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.next_movement_boundary(offset, MovementUnit::Identifier)
    }

    pub fn previous_subword_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.previous_movement_boundary(offset, MovementUnit::Subword)
    }

    pub fn next_subword_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.next_movement_boundary(offset, MovementUnit::Subword)
    }

    pub fn previous_symbol_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.previous_movement_boundary(offset, MovementUnit::Symbol)
    }

    pub fn next_symbol_boundary(&self, offset: CharOffset) -> EngineResult<CharOffset> {
        self.next_movement_boundary(offset, MovementUnit::Symbol)
    }

    /// M6B：移动一组选区的 head。
    ///
    /// `extend = false` 时移动后塌缩为 caret；`extend = true` 时保留 anchor，扩展/收缩选区。
    /// 该 API 只更新 selection，不提交文本事务，因此不污染 Undo 历史。
    pub fn move_selections(
        &mut self,
        selections: SelectionSet,
        direction: MovementDirection,
        unit: MovementUnit,
        extend: bool,
    ) -> EngineResult<SelectionSet> {
        self.validate_selection_set(&selections)?;

        let primary_index = selections.primary_index();
        let moved = selections
            .as_slice()
            .iter()
            .copied()
            .map(|selection| {
                let new_head = self.movement_boundary(selection.head(), direction, unit)?;

                Ok(if extend {
                    selection.with_head(new_head)
                } else {
                    Selection::caret(new_head)
                })
            })
            .collect::<EngineResult<Vec<_>>>()?;

        let moved = SelectionSet::new_with_primary(moved, primary_index);
        self.set_selection(moved.clone())?;
        Ok(moved)
    }

    /// M6B：移动当前 Buffer selection 的便捷入口。
    pub fn move_current_selection(
        &mut self,
        direction: MovementDirection,
        unit: MovementUnit,
        extend: bool,
    ) -> EngineResult<SelectionSet> {
        let selections = self.selection.clone();
        self.move_selections(selections, direction, unit, extend)
    }

    pub fn line_ending_style(&self) -> LineEndingStyle {
        self.storage.line_ending_style()
    }

    pub fn next_tab_stop(&self, display_column: DisplayColumn) -> DisplayColumn {
        next_tab_stop(display_column, self.config.tab.tab_width())
    }

    pub fn char_to_display_column(&self, offset: CharOffset) -> EngineResult<DisplayColumn> {
        char_to_display_column_in_text(&self.storage, &self.config, offset)
    }

    pub fn logical_to_display_column(
        &self,
        line: Line,
        column: LogicalColumn,
    ) -> EngineResult<DisplayColumn> {
        logical_to_display_column_in_text(&self.storage, &self.config, line, column)
    }

    pub fn display_to_logical_column(
        &self,
        line: Line,
        column: DisplayColumn,
    ) -> EngineResult<LogicalColumn> {
        display_to_logical_column_in_text(
            &self.storage,
            &self.config,
            line,
            column,
            self.config.display_width.affinity,
        )
    }

    pub fn display_to_logical_column_with_affinity(
        &self,
        line: Line,
        column: DisplayColumn,
        affinity: DisplayColumnAffinity,
    ) -> EngineResult<LogicalColumn> {
        display_to_logical_column_in_text(&self.storage, &self.config, line, column, affinity)
    }

    pub fn display_column_to_char(
        &self,
        line: Line,
        column: DisplayColumn,
    ) -> EngineResult<CharOffset> {
        let logical = self.display_to_logical_column(line, column)?;
        self.storage.position_to_char(Position::new(line, logical))
    }

    pub fn display_column_to_char_with_affinity(
        &self,
        line: Line,
        column: DisplayColumn,
        affinity: DisplayColumnAffinity,
    ) -> EngineResult<CharOffset> {
        let logical = self.display_to_logical_column_with_affinity(line, column, affinity)?;
        self.storage.position_to_char(Position::new(line, logical))
    }

    /// 创建绑定当前版本的不可变快照。
    pub fn snapshot(&self) -> Snapshot {
        Snapshot::new(self.storage.snapshot(), self.version, self.config.clone())
    }

    /// 判断给定版本是否已经相对当前 Buffer 过期。
    pub fn is_version_stale(&self, version: BufferVersion) -> bool {
        version != self.version
    }

    pub fn is_snapshot_stale(&self, snapshot: &Snapshot) -> bool {
        snapshot.version() != self.version
    }

    pub fn selection(&self) -> &SelectionSet {
        &self.selection
    }

    pub fn set_selection(&mut self, selection: SelectionSet) -> EngineResult<()> {
        self.validate_selection_set(&selection)?;
        self.selection = selection;
        Ok(())
    }

    pub fn selection_after_edit(
        &self,
        selection: &SelectionSet,
        changeset: &ChangeSet,
    ) -> SelectionSet {
        selection.map_through_changeset(changeset)
    }

    /// 在每个 selection 处插入文本；非空 selection 会被替换。
    pub fn insert_at_selections(
        &mut self,
        selections: SelectionSet,
        text: &str,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.replace_selection_ranges_with_metadata(
            selections,
            text,
            TransactionMetadata::new(TransactionSource::Keyboard)
                .with_description("insert at selections"),
        )
    }

    /// 用同一段文本替换每个 selection。
    pub fn replace_selections(
        &mut self,
        selections: SelectionSet,
        replacement: &str,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.replace_selection_ranges_with_metadata(
            selections,
            replacement,
            TransactionMetadata::new(TransactionSource::Command)
                .with_description("replace selections"),
        )
    }

    /// 删除所有非空 selection range；caret 本身不会删除任何字符。
    pub fn delete_selection_ranges(
        &mut self,
        selections: SelectionSet,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.replace_selection_ranges_with_metadata(
            selections,
            "",
            TransactionMetadata::new(TransactionSource::Delete)
                .with_description("delete selections"),
        )
    }

    /// 对每个 caret 执行 grapheme-safe Backspace；非空 selection 直接删除 selection range。
    pub fn delete_backward_at_selections(
        &mut self,
        selections: SelectionSet,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.validate_selection_set(&selections)?;

        let mut delete_targets = Vec::new();

        for selection in selections.as_slice() {
            if selection.is_caret() {
                let end = selection.head();
                let start = self.previous_grapheme_boundary(end)?;

                if start != end {
                    delete_targets.push(Selection::new(start, end));
                }
            } else {
                delete_targets.push(*selection);
            }
        }

        if delete_targets.is_empty() {
            self.set_selection(selections)?;
            return Ok(None);
        }

        self.replace_selection_ranges_with_metadata(
            SelectionSet::new(delete_targets),
            "",
            TransactionMetadata::new(TransactionSource::Delete)
                .with_description("delete backward at selections"),
        )
    }

    /// 对每个 caret 执行 grapheme-safe Delete；非空 selection 直接删除 selection range。
    pub fn delete_forward_at_selections(
        &mut self,
        selections: SelectionSet,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        self.validate_selection_set(&selections)?;

        let mut delete_targets = Vec::new();

        for selection in selections.as_slice() {
            if selection.is_caret() {
                let start = selection.head();
                let end = self.next_grapheme_boundary(start)?;

                if start != end {
                    delete_targets.push(Selection::new(start, end));
                }
            } else {
                delete_targets.push(*selection);
            }
        }

        if delete_targets.is_empty() {
            self.set_selection(selections)?;
            return Ok(None);
        }

        self.replace_selection_ranges_with_metadata(
            SelectionSet::new(delete_targets),
            "",
            TransactionMetadata::new(TransactionSource::Delete)
                .with_description("delete forward at selections"),
        )
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

        let before_text = self.text().into_owned();
        let before_selection = tx_before_selection.unwrap_or_else(|| self.selection.clone());
        let undo_edits = Self::build_inverse_edit_list(&before_text, &tx_edits)?;
        let redo_edits = tx_edits.clone();

        let (delta, changeset) = self.apply_edit_list(base_version, tx_edits)?;

        let after_selection = tx_after_selection
            .unwrap_or_else(|| before_selection.map_through_changeset(&changeset));
        self.selection = after_selection.clone();

        let after_text = self.text().into_owned();

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
        if self.slice_text(range)?.as_ref() == replacement {
            return Ok(());
        }

        let tx = Transaction::from_edits(
            self.version,
            vec![Edit::replace(range, replacement.to_string())],
        )?;

        self.apply_transaction(tx)?;
        Ok(())
    }

    fn replace_selection_ranges_with_metadata(
        &mut self,
        selections: SelectionSet,
        replacement: &str,
        metadata: TransactionMetadata,
    ) -> EngineResult<Option<(Delta, ChangeSet)>> {
        let selections = selections.normalized();
        self.validate_selection_set(&selections)?;

        let before_selection = selections.clone();
        let replacement_len = replacement.chars().count();
        let replacement = replacement.to_string();

        let mut edits = Vec::new();
        let mut after_selections = Vec::with_capacity(selections.len());
        let mut diff = 0isize;

        for selection in selections.as_slice() {
            let range = selection.range();
            let old_start = range.start().get() as isize;
            let old_end = range.end().get() as isize;
            let new_start = (old_start + diff).max(0) as usize;
            let new_head = CharOffset::new(new_start + replacement_len);

            let is_empty_noop = range.is_empty() && replacement.is_empty();
            let is_same_text_noop =
                !range.is_empty() && self.slice_text(range)?.as_ref() == replacement.as_str();

            if !is_empty_noop && !is_same_text_noop {
                edits.push(Edit::replace(range, replacement.clone()));
                diff += replacement_len as isize - (old_end - old_start);
            }

            after_selections.push(Selection::caret(new_head));
        }

        let after_selection = SelectionSet::new(after_selections);

        if edits.is_empty() {
            self.selection = after_selection;
            return Ok(None);
        }

        let tx = Transaction::from_edits(self.version, edits)?
            .with_metadata(metadata)
            .with_selection(Some(before_selection), Some(after_selection));

        self.apply_transaction(tx).map(Some)
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

        // 3. 全部成功后再一次性提交 storage / version。
        self.storage = new_storage;
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

    fn validate_selection_set(&self, selections: &SelectionSet) -> EngineResult<()> {
        for selection in selections.as_slice() {
            self.validate_selection_boundary(selection.anchor())?;
            self.validate_selection_boundary(selection.head())?;
        }

        Ok(())
    }

    fn validate_selection_boundary(&self, offset: CharOffset) -> EngineResult<()> {
        self.validate_edit_boundary(offset)?;
        self.validate_grapheme_boundary(offset)
    }

    /// 校验范围是否合法，超出文本字符长度返回错误。
    fn validate_range(&self, range: TextRange) -> EngineResult<()> {
        if range.end() > self.len_chars() {
            return Err(EditError::RangeOutOfBounds { range }.into());
        }

        Ok(())
    }

    /// 校验编辑边界是否合法，超出文本范围或落在 CRLF 中间时返回错误。
    fn validate_edit_boundary(&self, offset: CharOffset) -> EngineResult<()> {
        let value = offset.get();
        let len_chars = self.len_chars().get();

        if value > len_chars {
            return Err(CoordinateError::OutOfBounds(offset).into());
        }

        if is_crlf_middle(&self.storage, offset) {
            return Err(EditError::InvalidBoundary { offset }.into());
        }

        Ok(())
    }

    fn slice_text(&self, range: TextRange) -> EngineResult<Cow<'_, str>> {
        self.storage.slice_text(range)
    }

    /// 递增版本号，溢出时返回错误。
    fn bump_version(&mut self) -> EngineResult<()> {
        self.version = self.version.next().ok_or(EngineError::VersionOverflow)?;
        Ok(())
    }
}

fn char_to_display_column_in_text<T: TextRead>(
    storage: &T,
    config: &BufferConfig,
    offset: CharOffset,
) -> EngineResult<DisplayColumn> {
    let position = storage.char_to_position(offset)?;
    logical_to_display_column_in_text(storage, config, position.line(), position.column())
}

fn logical_to_display_column_in_text<T: TextRead>(
    storage: &T,
    config: &BufferConfig,
    line: Line,
    column: LogicalColumn,
) -> EngineResult<DisplayColumn> {
    let line_start = storage.line_start(line)?;
    let offset = storage.position_to_char(Position::new(line, column))?;
    let range = TextRange::new(line_start, offset)?;
    let text = storage.slice_text(range)?;

    Ok(DisplayColumn::new(display_width_of_text(
        text.as_ref(),
        config,
    )))
}

fn display_to_logical_column_in_text<T: TextRead>(
    storage: &T,
    config: &BufferConfig,
    line: Line,
    column: DisplayColumn,
    affinity: DisplayColumnAffinity,
) -> EngineResult<LogicalColumn> {
    let line_start = storage.line_start(line)?;
    let line_end = line_content_end_for_storage(storage, line)?;
    let range = TextRange::new(line_start, line_end)?;
    let text = storage.slice_text(range)?;
    let target = column.get();
    let mut current_display = 0usize;
    let mut current_logical = 0usize;

    if target == 0 {
        return Ok(LogicalColumn::ZERO);
    }

    for ch in text.chars() {
        let next_display = advance_display_column(current_display, ch, config);
        let next_logical = current_logical + 1;

        if target == current_display {
            return Ok(LogicalColumn::new(current_logical));
        }

        if target == next_display {
            return Ok(LogicalColumn::new(next_logical));
        }

        if target > current_display && target < next_display {
            return Ok(LogicalColumn::new(match affinity {
                DisplayColumnAffinity::Previous => current_logical,
                DisplayColumnAffinity::Next => next_logical,
                DisplayColumnAffinity::Nearest => {
                    let distance_to_previous = target - current_display;
                    let distance_to_next = next_display - target;

                    if distance_to_previous <= distance_to_next {
                        current_logical
                    } else {
                        next_logical
                    }
                }
            }));
        }

        current_display = next_display;
        current_logical = next_logical;
    }

    Ok(LogicalColumn::new(current_logical))
}

fn line_content_end_for_storage<T: TextRead>(storage: &T, line: Line) -> EngineResult<CharOffset> {
    let line_start = storage.line_start(line)?.get();
    let mut next_line_start = if line.get() + 1 < storage.line_count() {
        storage.line_start(Line::new(line.get() + 1))?.get()
    } else {
        storage.len_chars().get()
    };

    if next_line_start > line_start
        && storage.char_at(CharOffset::new(next_line_start - 1)) == Some('\n')
    {
        next_line_start -= 1;

        if next_line_start > line_start
            && storage.char_at(CharOffset::new(next_line_start - 1)) == Some('\r')
        {
            next_line_start -= 1;
        }
    }

    Ok(CharOffset::new(next_line_start))
}

fn display_width_of_text(text: &str, config: &BufferConfig) -> usize {
    text.chars().fold(0usize, |display_column, ch| {
        advance_display_column(display_column, ch, config)
    })
}

fn advance_display_column(display_column: usize, ch: char, config: &BufferConfig) -> usize {
    if ch == '\t' {
        next_tab_stop(DisplayColumn::new(display_column), config.tab.tab_width()).get()
    } else {
        display_column + config.display_width.char_width(ch)
    }
}

fn next_tab_stop(display_column: DisplayColumn, tab_width: usize) -> DisplayColumn {
    let current = display_column.get();
    let remainder = current % tab_width;
    let delta = if remainder == 0 {
        tab_width
    } else {
        tab_width - remainder
    };

    DisplayColumn::new(current + delta)
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

fn is_crlf_middle<T: TextRead>(storage: &T, offset: CharOffset) -> bool {
    let value = offset.get();

    value > 0
        && value < storage.len_chars().get()
        && storage.char_at(CharOffset::new(value - 1)) == Some('\r')
        && storage.char_at(offset) == Some('\n')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MovementTokenSpan {
    start: CharOffset,
    end: CharOffset,
}

impl MovementTokenSpan {
    fn new(start: usize, end: usize) -> Self {
        Self {
            start: CharOffset::new(start),
            end: CharOffset::new(end),
        }
    }
}

fn movement_boundary_in_text<T: TextRead>(
    storage: &T,
    policy: WordBoundaryPolicy,
    offset: CharOffset,
    direction: MovementDirection,
    unit: MovementUnit,
) -> EngineResult<CharOffset> {
    validate_movement_offset(storage, offset)?;

    match unit {
        MovementUnit::Grapheme => match direction {
            MovementDirection::Previous => storage.previous_grapheme_boundary(offset),
            MovementDirection::Next => storage.next_grapheme_boundary(offset),
        },
        MovementUnit::Word => {
            let text = storage.text();
            let spans = unicode_word_spans(text.as_ref());
            Ok(boundary_from_token_spans(
                &spans,
                offset,
                direction,
                storage.len_chars(),
            ))
        }
        MovementUnit::Identifier => {
            let text = storage.text();
            let spans = identifier_spans(text.as_ref(), policy);
            Ok(boundary_from_token_spans(
                &spans,
                offset,
                direction,
                storage.len_chars(),
            ))
        }
        MovementUnit::Subword => {
            let text = storage.text();
            let spans = subword_spans(text.as_ref());
            Ok(boundary_from_token_spans(
                &spans,
                offset,
                direction,
                storage.len_chars(),
            ))
        }
        MovementUnit::Symbol => {
            let text = storage.text();
            let spans = symbol_spans(text.as_ref(), policy);
            Ok(boundary_from_token_spans(
                &spans,
                offset,
                direction,
                storage.len_chars(),
            ))
        }
    }
}

fn validate_movement_offset<T: TextRead>(storage: &T, offset: CharOffset) -> EngineResult<()> {
    if offset > storage.len_chars() {
        return Err(CoordinateError::OutOfBounds(offset).into());
    }

    if is_crlf_middle(storage, offset) {
        return Err(EditError::InvalidBoundary { offset }.into());
    }

    if !storage.is_grapheme_boundary(offset)? {
        return Err(CoordinateError::InvalidGraphemeBoundary(offset).into());
    }

    Ok(())
}

fn boundary_from_token_spans(
    spans: &[MovementTokenSpan],
    offset: CharOffset,
    direction: MovementDirection,
    text_len: CharOffset,
) -> CharOffset {
    match direction {
        MovementDirection::Next => spans
            .iter()
            .find_map(|span| {
                if offset < span.start {
                    Some(span.start)
                } else if offset < span.end {
                    Some(span.end)
                } else {
                    None
                }
            })
            .unwrap_or(text_len),
        MovementDirection::Previous => spans
            .iter()
            .rev()
            .find_map(|span| {
                if offset > span.end {
                    Some(span.end)
                } else if offset > span.start {
                    Some(span.start)
                } else {
                    None
                }
            })
            .unwrap_or(CharOffset::ZERO),
    }
}

fn unicode_word_spans(text: &str) -> Vec<MovementTokenSpan> {
    let mut spans: Vec<MovementTokenSpan> = Vec::new();

    for (byte_start, word) in text.unicode_word_indices() {
        let start = text[..byte_start].chars().count();
        let end = start + word.chars().count();

        if let Some(previous) = spans.last_mut() {
            if previous.end.get() == start {
                previous.end = CharOffset::new(end);
                continue;
            }
        }

        spans.push(MovementTokenSpan::new(start, end));
    }

    spans
}

fn identifier_spans(text: &str, policy: WordBoundaryPolicy) -> Vec<MovementTokenSpan> {
    contiguous_spans_by_char(text, |ch| policy.is_identifier_continue(ch))
}

fn symbol_spans(text: &str, policy: WordBoundaryPolicy) -> Vec<MovementTokenSpan> {
    contiguous_spans_by_char(text, |ch| policy.is_symbol_char(ch))
}

fn contiguous_spans_by_char(
    text: &str,
    mut predicate: impl FnMut(char) -> bool,
) -> Vec<MovementTokenSpan> {
    let mut spans = Vec::new();
    let mut current_start = None;

    for (idx, ch) in text.chars().enumerate() {
        if predicate(ch) {
            current_start.get_or_insert(idx);
        } else if let Some(start) = current_start.take() {
            spans.push(MovementTokenSpan::new(start, idx));
        }
    }

    if let Some(start) = current_start {
        spans.push(MovementTokenSpan::new(start, text.chars().count()));
    }

    spans
}

fn subword_spans(text: &str) -> Vec<MovementTokenSpan> {
    let chars = text.chars().collect::<Vec<_>>();
    let mut spans = Vec::new();
    let mut current_start = None;

    for idx in 0..chars.len() {
        let ch = chars[idx];

        if !is_subword_body_char(ch) {
            if let Some(start) = current_start.take() {
                spans.push(MovementTokenSpan::new(start, idx));
            }
            continue;
        }

        match current_start {
            None => current_start = Some(idx),
            Some(start) => {
                let previous = chars[idx - 1];
                let next = chars.get(idx + 1).copied();

                if should_start_new_subword(previous, ch, next) {
                    spans.push(MovementTokenSpan::new(start, idx));
                    current_start = Some(idx);
                }
            }
        }
    }

    if let Some(start) = current_start {
        spans.push(MovementTokenSpan::new(start, chars.len()));
    }

    spans
}

fn is_subword_body_char(ch: char) -> bool {
    ch.is_alphanumeric() || is_combining_mark_for_movement(ch)
}

fn should_start_new_subword(previous: char, current: char, next: Option<char>) -> bool {
    if is_combining_mark_for_movement(current) || is_combining_mark_for_movement(previous) {
        return false;
    }

    (previous.is_lowercase() && current.is_uppercase())
        || (previous.is_alphabetic() && current.is_numeric())
        || (previous.is_numeric() && current.is_alphabetic())
        || (previous.is_uppercase()
            && current.is_uppercase()
            && next.is_some_and(char::is_lowercase))
}

fn is_combining_mark_for_movement(ch: char) -> bool {
    matches!(
        ch as u32,
        0x0300..=0x036F
            | 0x1AB0..=0x1AFF
            | 0x1DC0..=0x1DFF
            | 0x20D0..=0x20FF
            | 0xFE20..=0xFE2F
    )
}
