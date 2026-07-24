//! Selection 编辑入口：把多光标、多选区插入、替换和删除归一化为单个事务。
//!
//! 本文件负责 selection 语义到 EditList 的映射，不实现底层提交原子性，也不绕过 Buffer 边界校验。

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::{
    ByteOffset, EditOutcome, EngineError, EngineResult, Line, MovementDirection, MovementUnit,
    Selection, SelectionSet,
    config::TabConfig,
    position_map::OffsetShift,
    storage::TextRead,
    transaction::{Edit, Transaction, TransactionMetadata, TransactionSource},
};

use crate::buffer::Buffer;

impl Buffer {
    /// 在每个 selection 处插入文本；非空 selection 会被替换。
    pub fn insert_at_selections(
        &mut self,
        selections: &SelectionSet,
        text: &str,
        metadata: TransactionMetadata,
    ) -> EngineResult<EditOutcome> {
        self.replace_selection_ranges_with_metadata(selections, text, metadata)
    }

    /// 用同一段文本替换每个 selection。
    pub fn replace_selections(
        &mut self,
        selections: &SelectionSet,
        replacement: &str,
        metadata: TransactionMetadata,
    ) -> EngineResult<EditOutcome> {
        self.replace_selection_ranges_with_metadata(selections, replacement, metadata)
    }

    /// Tab 缩进，按"标准 IDE"语义分流：
    ///
    /// - **所有 selection 都是 caret**：在每个 caret 原位插入一个"软 Tab"。
    ///   `insert_spaces = true` 时按 `indent_width − (display_column mod indent_width)` 计算空格数，让 caret 落到下一个 `indent_width` 列位；
    /// insert_spaces = false` 时插入一个 `'\t'`。
    /// - **存在任何非空 selection**：将选区涉及的所有行做行块缩进（不替换选中内容）。
    /// 多 selection 共占同一行只缩进一次。
    pub fn indent_at_selections(&mut self, selections: &SelectionSet) -> EngineResult<EditOutcome> {
        self.ensure_writable()?;
        self.validate_selection_set(selections)?;

        let selections = selections.normalized();
        let all_carets = selections.as_slice().iter().all(|s| s.is_caret());
        let metadata =
            TransactionMetadata::new(TransactionSource::Programmatic).with_description("增加缩进");

        if all_carets {
            let mut targets = Vec::with_capacity(selections.as_slice().len());
            for selection in selections.as_slice() {
                let text = self.soft_tab_text_for_caret(selection.head())?;
                targets.push((*selection, Arc::from(text)));
            }
            return self.apply_targeted_edits_with_metadata(targets, &selections, metadata);
        }

        let lines = collect_touched_lines(&self.storage, &selections)?;
        let indent_text: Arc<str> = Arc::from(indent_string(&self.config.tab));

        let mut targets = Vec::with_capacity(lines.len());
        for line in lines {
            let line_start = self.storage.line_start(line)?;
            targets.push((Selection::caret(line_start), Arc::clone(&indent_text)));
        }

        self.apply_targeted_edits_with_metadata(targets, &selections, metadata)
    }

    /// 计算 caret 处应该插入的"软 Tab"文本（依据 `BufferConfig.tab`）。
    fn soft_tab_text_for_caret(&self, caret: ByteOffset) -> EngineResult<String> {
        let tab = &self.config.tab;
        if !tab.insert_spaces {
            return Ok("\t".to_string());
        }
        let caret_char = self.storage.byte_to_char(caret)?;
        let display_col = self.char_to_display_column(caret_char)?.get();
        let width = tab.indent_width();
        let needed = width - (display_col % width);
        Ok(" ".repeat(needed))
    }

    /// 反向缩进：在每行行首移除最多一个缩进单位的前导空白。
    ///
    /// 规则：行首若以 `'\t'` 起头则只删除该 tab；否则按 `indent_width` 删除连续的前导空格。
    /// 行首没有空白则跳过；所有行都没有可删空白时不产生事务。
    pub fn outdent_at_selections(
        &mut self,
        selections: &SelectionSet,
    ) -> EngineResult<EditOutcome> {
        self.ensure_writable()?;
        self.validate_selection_set(selections)?;

        let lines = collect_touched_lines(&self.storage, selections)?;
        let indent_width = self.config.tab.indent_width();

        let mut targets = Vec::new();
        for line in lines {
            let line_start = self.storage.line_start(line)?;
            if let Some(end) = leading_indent_end(&self.storage, line_start, indent_width)?
                && end > line_start
            {
                targets.push(Selection::new(line_start, end));
            }
        }

        if targets.is_empty() {
            return Ok(EditOutcome::unchanged(selections.clone()));
        }

        self.apply_targeted_edits_with_metadata(
            targets
                .into_iter()
                .map(|target| (target, Arc::<str>::from("")))
                .collect(),
            selections,
            TransactionMetadata::new(TransactionSource::Programmatic).with_description("减少缩进"),
        )
    }

    /// 删除的唯一引擎入口。三种语义按 `caret_motion` 分流：
    ///
    /// - `caret_motion = Some((dir, unit))`：caret 沿 `dir` 推一个 `unit` 边界，把 `[caret, boundary)` 删掉；
    /// 非空 selection 整段删（`caret_motion` 无效）。
    /// - `caret_motion = None`：caret no-op，只删非空 selection。
    ///
    /// 事务描述按 `(dir, unit)` 落到 `(direction, unit)` 矩阵；`None` 时写「删除所选内容」。
    pub fn delete_at_selections(
        &mut self,
        selections: &SelectionSet,
        caret_motion: Option<(MovementDirection, MovementUnit)>,
        metadata: TransactionMetadata,
    ) -> EngineResult<EditOutcome> {
        self.ensure_writable()?;
        self.validate_selection_set(selections)?;

        let mut delete_targets = Vec::new();

        for selection in selections.as_slice() {
            if selection.is_caret() {
                let Some((direction, unit)) = caret_motion else {
                    // direction = None → caret no-op，跳过本条 selection。
                    continue;
                };
                let head = selection.head();
                // head 是 ByteOffset，movement_boundary 仍按 char 计算，跨边界转换。
                let head_char = self.storage.byte_to_char(head)?;
                let boundary_char = self.movement_boundary(head_char, direction, unit)?;
                // movement_boundary 接 impl Into<Motion>，MovementUnit 自动包装为 ByUnit。
                let boundary = self.storage.char_to_byte(boundary_char)?;
                let range_selection = match direction {
                    MovementDirection::Previous => Selection::new(boundary, head),
                    MovementDirection::Next => Selection::new(head, boundary),
                };

                if !range_selection.range().is_empty() {
                    delete_targets.push(range_selection);
                }
            } else {
                delete_targets.push(*selection);
            }
        }

        if delete_targets.is_empty() {
            return Ok(EditOutcome::unchanged(selections.clone()));
        }

        self.replace_selection_ranges_with_metadata(
            &SelectionSet::new(delete_targets),
            "",
            metadata,
        )
    }

    pub(in crate::buffer) fn replace_selection_ranges_with_metadata(
        &mut self,
        selections: &SelectionSet,
        replacement: &str,
        metadata: TransactionMetadata,
    ) -> EngineResult<EditOutcome> {
        self.ensure_writable()?;

        let selections = selections.normalized();
        self.validate_selection_set(&selections)?;

        // ByteOffset 深核：用 byte 长度
        let replacement_len = replacement.len();
        let replacement: Arc<str> = Arc::from(replacement);

        let mut edits = Vec::new();
        let mut after_selections = Vec::with_capacity(selections.len());
        let mut shift = OffsetShift::ZERO;

        for selection in selections.as_slice() {
            let range = selection.range();
            let new_start = shift
                .apply_old_to_new(range.start())
                .ok_or_else(|| offset_arithmetic_bug("replace_selection_ranges_with_metadata"))?;
            let new_head = new_start
                .checked_add(replacement_len)
                .ok_or_else(|| offset_arithmetic_bug("replace_selection_ranges_with_metadata"))?;

            let is_empty_noop = range.is_empty() && replacement.is_empty();
            // 流式比较，零拷贝。range 为空或长度不等都视为「不是同文 noop」。
            // 长度不等无须比内容，空 range 由 is_empty_noop 单独覆盖。
            let is_same_text_noop = if range.is_empty() || range.len() != replacement.len() {
                false
            } else {
                let mut consumed = 0usize;
                let mut equal = true;
                for chunk in self.storage.chunks(range)? {
                    let end = consumed + chunk.len();
                    if &replacement.as_bytes()[consumed..end] != chunk.as_bytes() {
                        equal = false;
                        break;
                    }
                    consumed = end;
                }
                equal && consumed == replacement.len()
            };

            if !is_empty_noop && !is_same_text_noop {
                edits.push(Edit::replace(range, Arc::clone(&replacement)));
                shift = shift
                    .after_edit(range.len(), replacement_len)
                    .ok_or_else(|| {
                        offset_arithmetic_bug("replace_selection_ranges_with_metadata")
                    })?;
            }

            after_selections.push(Selection::caret(new_head));
        }

        let after_selection = SelectionSet::new(after_selections);

        if edits.is_empty() {
            return Ok(EditOutcome::unchanged(after_selection));
        }

        let tx = Transaction::from_edits(self.version, edits)?.with_metadata(metadata);

        let transaction = self.apply_transaction_outcome(tx)?;
        Ok(EditOutcome::edited(transaction, after_selection))
    }

    /// 按每条 edit 自带替换文本的方式提交一次事务。
    ///
    /// `targets` 必须按 `selection.range().start()` 升序、且范围互不重叠；调用方负责保证。
    /// 与 `replace_selection_ranges_with_metadata` 的区别是这里每条 edit 携带各自的 `replacement`，
    /// 适合"软 Tab 因列位不同长度不一"或"多 caret 各插不同文本"的场景。
    pub(in crate::buffer) fn apply_targeted_edits_with_metadata(
        &mut self,
        targets: Vec<(Selection, Arc<str>)>,
        before_selection: &SelectionSet,
        metadata: TransactionMetadata,
    ) -> EngineResult<EditOutcome> {
        self.ensure_writable()?;

        for (selection, _) in &targets {
            self.validate_range(selection.range())?;
        }

        let mut edits = Vec::with_capacity(targets.len());
        for (selection, text) in &targets {
            let range = selection.range();
            if !(range.is_empty() && text.is_empty()) {
                edits.push(Edit::replace(range, Arc::clone(text)));
            }
        }

        if edits.is_empty() {
            return Ok(EditOutcome::unchanged(before_selection.clone()));
        }

        let tx = Transaction::from_edits(self.version, edits)?.with_metadata(metadata);

        let transaction = self.apply_transaction_outcome(tx)?;
        let after_selection = transaction
            .changeset()
            .position_map()
            .map_selection_set(before_selection);
        Ok(EditOutcome::edited(transaction, after_selection))
    }
}

fn offset_arithmetic_bug(location: &'static str) -> EngineError {
    EngineError::EngineBug {
        location,
        detail: "映射 selection 编辑时字节偏移溢出".to_string(),
    }
}

/// 收集 selections 实际触及的逻辑行集合，去重并按行号升序返回。
///
/// 注意：当非空 selection 的结束端点恰好落在下一行行首时，认为用户并未"选中"那一行，
/// 因此跳过末端那一行，避免缩进时多吃一行。
fn collect_touched_lines<T: TextRead>(
    storage: &T,
    selections: &SelectionSet,
) -> EngineResult<Vec<Line>> {
    let mut lines = BTreeSet::new();
    for selection in selections.as_slice() {
        let range = selection.range();
        let start_line = storage.byte_to_position(range.start())?.line();
        let mut end_line = storage.byte_to_position(range.end())?.line();

        if !range.is_empty() && end_line > start_line {
            // 选区终点正好落在下一行行首：那一行其实未被选中。
            let end_line_start = storage.line_start(end_line)?;
            if end_line_start == range.end() {
                end_line = Line::new(end_line.get() - 1);
            }
        }

        for line in start_line.get()..=end_line.get() {
            lines.insert(Line::new(line));
        }
    }
    Ok(lines.into_iter().collect())
}

fn indent_string(tab: &TabConfig) -> String {
    if tab.insert_spaces {
        " ".repeat(tab.indent_width())
    } else {
        "\t".to_string()
    }
}

/// 行首扫描：返回应当被"反缩进"删除的前导空白末尾 ByteOffset。
///
/// 行首若是 `\t`，只删该 tab；否则按 `indent_width` 删除连续的前导空格。返回 `None`
/// 表示行首无可删空白（调用方据此跳过该行）。
fn leading_indent_end<T: TextRead>(
    storage: &T,
    line_start: ByteOffset,
    indent_width: usize,
) -> EngineResult<Option<ByteOffset>> {
    let first = match storage.char_at_byte(line_start) {
        Some(ch) => ch,
        None => return Ok(None),
    };

    if first == '\t' {
        // 单个 tab 是一个 ASCII 字节，且自身就是一个 grapheme。
        return Ok(Some(storage.next_grapheme_boundary(line_start)?));
    }

    if first != ' ' {
        return Ok(None);
    }

    // 删除最多 indent_width 个连续空格；空格是 ASCII，可直接按字节推进。
    let len_bytes = storage.len_bytes().get();
    let mut cursor = line_start.get();
    let mut removed = 0usize;
    while removed < indent_width && cursor < len_bytes {
        let next_offset = ByteOffset::new(cursor);
        match storage.char_at_byte(next_offset) {
            Some(' ') => {
                cursor += 1;
                removed += 1;
            }
            _ => break,
        }
    }

    Ok(Some(ByteOffset::new(cursor)))
}
