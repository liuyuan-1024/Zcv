use gpui::{
    App, Application, Bounds, Context, Div, FocusHandle, IntoElement, KeyBinding, KeyDownEvent,
    Render, Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb,
};

use zom_engine::{
    Buffer, BufferConfig, BufferVersion, ByteOffset, CharOffset, DisplayColumn,
    DisplayColumnAffinity, Edit, EditList, EngineResult, Line, LogicalColumn, Position, Selection,
    SelectionSet, Snapshot, TextRange, Transaction, TransactionMergePolicy, TransactionMetadata,
    TransactionSource, Utf16Position,
};

// M6 testbed：必须是 M5B testbed 的 superset。
// 保留 M5B 的输入 / Delete / Home / End / Save / Reset / 批量事务 / Delta / ChangeSet / Undo / Redo / Snapshot / Rope metrics / IDE 坐标 / 视觉列能力，
// 再叠加 M6 SelectionSet：anchor/head、范围选区、多光标、选区替换、Undo/Redo 恢复选区。
actions!(
    m6_testbed,
    [
        Backspace,
        DeleteForward,
        Enter,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        Home,
        End,
        Save,
        Reset,
        SelectAll,
        ClearSelection,
        MultiCursorDemo,
        MultiSelectionDemo,
        ReplaceSelectionsProbe,
        BatchEdit,
        Undo,
        Redo,
        MergeDemo,
        CaptureSnapshot,
        InsertLongLine,
        InsertUnicodeProbe,
        InsertDisplayProbe
    ]
);

const INITIAL_TEXT: &str = concat!(
    "🚀 Zom Engine M6 中文测试台
",
    "
",
    "[A] 这是锚点A，[B] 这是锚点B。
",
    "可以输入、回车、退格、Delete、左右移动光标。
",
    "Home / End 可跳到当前行首尾。
",
    "Cmd-S 标记已保存，Cmd-R 重置文本。
",
    "Cmd-B 触发批量事务修改，连续字符输入会合并成一个撤销步骤，Cmd-M 触发一次合并到上一步的事务。
",
    "Cmd-Z / Ctrl-Z 撤销，Cmd-Shift-Z / Ctrl-Y 重做。
",
    "Cmd-K 捕获当前快照，并观察后续编辑后快照是否过期。
",
    "Cmd-L 在当前光标插入一段中等长度的单行文本，用于快速验证 RopeyStorage 的局部插入与指标更新。
",
    "Cmd-U 插入 Unicode / 字素 / CRLF 探针文本；Cmd-T 插入 Tab / CJK / emoji 视觉列探针。
",
    "M6 额外显示 SelectionSet、primary selection、anchor/head、多光标和选区范围，并保留 M5B 的 IDE 坐标 / 视觉列面板。
",
    "M6 仍然保持字素安全光标移动，并支持 Shift-Left/Right 扩展选区，Cmd-A 全选，Esc 清除为单光标。
",
    "这是一行 CRLF 探针，会让换行风格进入混合状态。
",
    "所有写入都走事务；黄色区间是变更范围，紫色是 selection，蓝色竖线是 caret，A/B 会跟随 ChangeSet 映射。
",
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeGroup {
    Typing,
    Backspace,
    DeleteForward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HorizontalDirection {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineEdge {
    Start,
    End,
}

#[derive(Debug, Clone)]
struct CoordinateProbe {
    char_offset: CharOffset,
    byte_offset: ByteOffset,
    utf16_position: Utf16Position,
    char_from_byte: CharOffset,
    char_from_utf16: CharOffset,
    byte_from_utf16: ByteOffset,
    utf16_from_byte: Utf16Position,
    display_column: DisplayColumn,
    next_tab_stop: DisplayColumn,
    logical_from_display: LogicalColumn,
    logical_from_display_previous: LogicalColumn,
    logical_from_display_next: LogicalColumn,
    char_from_display: CharOffset,
    grapheme_boundary: bool,
    previous_grapheme: CharOffset,
    next_grapheme: CharOffset,
}

pub struct M6Testbed {
    buffer: Buffer,
    cursor: CharOffset,
    anchor_a: CharOffset,
    anchor_b: CharOffset,
    last_changed_ranges: Vec<TextRange>,
    last_delta: Option<(BufferVersion, BufferVersion, usize)>,
    last_history_event: Option<String>,
    pinned_snapshot: Option<Snapshot>,
    merge_group: Option<MergeGroup>,
    focus_handle: FocusHandle,
    last_error: Option<String>,
}

impl M6Testbed {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (buffer, anchor_a, anchor_b) = initial_buffer_and_anchors();

        Self {
            buffer,
            cursor: CharOffset::ZERO,
            anchor_a,
            anchor_b,
            last_changed_ranges: Vec::new(),
            last_delta: None,
            last_history_event: None,
            pinned_snapshot: None,
            merge_group: None,
            focus_handle: cx.focus_handle(),
            last_error: None,
        }
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        let (buffer, anchor_a, anchor_b) = initial_buffer_and_anchors();

        self.buffer = buffer;
        self.cursor = CharOffset::ZERO;
        self.anchor_a = anchor_a;
        self.anchor_b = anchor_b;
        self.last_changed_ranges.clear();
        self.last_delta = None;
        self.last_history_event = Some("重置".to_string());
        self.pinned_snapshot = None;
        self.merge_group = None;
        self.last_error = None;

        cx.notify();
    }

    fn mark_saved(&mut self, cx: &mut Context<Self>) {
        self.buffer.mark_saved();
        self.last_history_event = Some("标记已保存".to_string());
        self.merge_group = None;
        self.last_error = None;
        cx.notify();
    }

    fn capture_snapshot(&mut self, cx: &mut Context<Self>) {
        self.pinned_snapshot = Some(self.buffer.snapshot());
        self.last_history_event = Some("捕获快照".to_string());
        self.merge_group = None;
        self.last_error = None;
        cx.notify();
    }

    fn submit_edits(
        &mut self,
        edits: Vec<Edit>,
        metadata: TransactionMetadata,
        cx: &mut Context<Self>,
    ) -> bool {
        let edit_list = match EditList::new(edits) {
            Ok(edit_list) => edit_list,
            Err(err) => {
                self.last_error = Some(err.to_string());
                cx.notify();
                return false;
            }
        };

        let before_selection = self.buffer.selection().clone();

        let tx = match Transaction::new(self.buffer.version(), edit_list) {
            Ok(tx) => tx
                .with_metadata(metadata)
                .with_selection(Some(before_selection), None),
            Err(err) => {
                self.last_error = Some(err.to_string());
                cx.notify();
                return false;
            }
        };

        self.apply_transaction(tx, cx)
    }

    fn submit_selection_replacement(
        &mut self,
        selections: SelectionSet,
        replacement: &str,
        metadata: TransactionMetadata,
        cx: &mut Context<Self>,
    ) -> bool {
        let selections = selections.normalized();
        if let Err(err) = self.buffer.set_selection(selections.clone()) {
            self.merge_group = None;
            self.last_error = Some(err.to_string());
            cx.notify();
            return false;
        }

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

            if !(range.is_empty() && replacement.is_empty()) {
                edits.push(Edit::replace(range, replacement.clone()));
                diff += replacement_len as isize - (old_end - old_start);
            }

            after_selections.push(Selection::caret(new_head));
        }

        let after_selection = SelectionSet::new(after_selections);

        if edits.is_empty() {
            if let Err(err) = self.buffer.set_selection(after_selection) {
                self.last_error = Some(err.to_string());
            } else {
                self.cursor = self.cursor_from_engine_selection();
                self.last_error = None;
            }
            cx.notify();
            return false;
        }

        let tx = match Transaction::from_edits(self.buffer.version(), edits) {
            Ok(tx) => tx
                .with_metadata(metadata)
                .with_selection(Some(selections), Some(after_selection)),
            Err(err) => {
                self.merge_group = None;
                self.last_error = Some(err.to_string());
                cx.notify();
                return false;
            }
        };

        self.apply_transaction(tx, cx)
    }

    fn apply_transaction(&mut self, tx: Transaction, cx: &mut Context<Self>) -> bool {
        let applied = match self.buffer.apply_transaction(tx) {
            Ok((delta, changeset)) => {
                self.last_error = None;
                self.last_delta = Some((
                    delta.old_version,
                    delta.new_version,
                    delta.edits.as_slice().len(),
                ));

                // M2 核心体感：锚点通过 ChangeSet 跟随文本变化。
                // M6 起，光标来源以 Buffer 内部 SelectionSet 为准。
                self.anchor_a = changeset.map_position(self.anchor_a);
                self.anchor_b = changeset.map_position(self.anchor_b);
                self.last_changed_ranges = changeset.changed_ranges();
                self.cursor = self.cursor_from_engine_selection();

                self.last_history_event = Some("应用事务".to_string());
                true
            }
            Err(err) => {
                self.last_error = Some(err.to_string());
                false
            }
        };

        cx.notify();
        applied
    }

    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let metadata = self
            .metadata_for_group(MergeGroup::Typing, TransactionSource::Keyboard)
            .with_description(format!("插入 {text:?}"));

        if self.submit_selection_replacement(self.buffer.selection().clone(), text, metadata, cx) {
            self.merge_group = Some(MergeGroup::Typing);
        }
    }

    fn backspace(&mut self, cx: &mut Context<Self>) {
        match self.delete_targets_for_backspace() {
            Ok(targets) if !targets.is_empty() => {
                let metadata = self
                    .metadata_for_group(MergeGroup::Backspace, TransactionSource::Delete)
                    .with_description("多光标 Backspace");

                if self.submit_selection_replacement(SelectionSet::new(targets), "", metadata, cx) {
                    self.merge_group = Some(MergeGroup::Backspace);
                }
            }
            Ok(_) => {
                self.merge_group = None;
                self.last_error = None;
                cx.notify();
            }
            Err(err) => {
                self.merge_group = None;
                self.last_error = Some(err.to_string());
                cx.notify();
            }
        }
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
        match self.delete_targets_for_delete_forward() {
            Ok(targets) if !targets.is_empty() => {
                let metadata = self
                    .metadata_for_group(MergeGroup::DeleteForward, TransactionSource::Delete)
                    .with_description("多光标 Delete");

                if self.submit_selection_replacement(SelectionSet::new(targets), "", metadata, cx) {
                    self.merge_group = Some(MergeGroup::DeleteForward);
                }
            }
            Ok(_) => {
                self.merge_group = None;
                self.last_error = None;
                cx.notify();
            }
            Err(err) => {
                self.merge_group = None;
                self.last_error = Some(err.to_string());
                cx.notify();
            }
        }
    }

    fn delete_targets_for_backspace(&self) -> EngineResult<Vec<Selection>> {
        let mut targets = Vec::new();

        for selection in self.buffer.selection().as_slice() {
            if selection.is_caret() {
                let end = selection.head();
                let start = self.buffer.previous_grapheme_boundary(end)?;
                if start != end {
                    targets.push(Selection::new(start, end));
                }
            } else {
                targets.push(*selection);
            }
        }

        Ok(targets)
    }

    fn delete_targets_for_delete_forward(&self) -> EngineResult<Vec<Selection>> {
        let mut targets = Vec::new();

        for selection in self.buffer.selection().as_slice() {
            if selection.is_caret() {
                let start = selection.head();
                let end = self.buffer.next_grapheme_boundary(start)?;
                if start != end {
                    targets.push(Selection::new(start, end));
                }
            } else {
                targets.push(*selection);
            }
        }

        Ok(targets)
    }

    fn batch_edit(&mut self, cx: &mut Context<Self>) {
        let marker = Edit::insert(CharOffset::ZERO, "[批量标记] ".to_string()).unwrap();

        // 避免在 cursor=0 时制造两个同 offset 的插入，让 testbed 的视觉结果更可预期。
        let sparkle_offset = if self.cursor == CharOffset::ZERO {
            self.buffer.len_chars()
        } else {
            self.cursor
        };
        let sparkle = Edit::insert(sparkle_offset, "✨".to_string()).unwrap();

        self.merge_group = None;
        self.submit_edits(
            vec![marker, sparkle],
            TransactionMetadata::new(TransactionSource::Command).with_description("批量事务"),
            cx,
        );
    }

    fn insert_long_line(&mut self, cx: &mut Context<Self>) {
        let payload = format!("[M5B Ropey 长行探针] {}\n", "中🙂Ropey".repeat(128));

        match Edit::insert(self.cursor, payload) {
            Ok(edit) => {
                self.merge_group = None;
                self.submit_edits(
                    vec![edit],
                    TransactionMetadata::new(TransactionSource::Command)
                        .with_description("M5B 插入长行探针"),
                    cx,
                );
            }
            Err(err) => {
                self.merge_group = None;
                self.last_error = Some(err.to_string());
                cx.notify();
            }
        }
    }

    fn insert_unicode_probe(&mut self, cx: &mut Context<Self>) {
        let payload = "[M5A Unicode 探针] 👨‍👩‍👧‍👦 e\u{301} café 中🙂 后面是 CRLF\r\n[M5A CRLF 探针]\r\n";

        match Edit::insert(self.cursor, payload.to_string()) {
            Ok(edit) => {
                self.merge_group = None;
                self.submit_edits(
                    vec![edit],
                    TransactionMetadata::new(TransactionSource::Command)
                        .with_description("M5A 插入 Unicode 探针"),
                    cx,
                );
            }
            Err(err) => {
                self.merge_group = None;
                self.last_error = Some(err.to_string());
                cx.notify();
            }
        }
    }

    fn insert_display_probe(&mut self, cx: &mut Context<Self>) {
        let payload = "[M5B 视觉列探针]\n列: 0123456789012345\nTab: a\tb\t中😀e\u{301}x\n";

        match Edit::insert(self.cursor, payload.to_string()) {
            Ok(edit) => {
                self.merge_group = None;
                self.submit_edits(
                    vec![edit],
                    TransactionMetadata::new(TransactionSource::Command)
                        .with_description("M5B 插入视觉列探针"),
                    cx,
                );
            }
            Err(err) => {
                self.merge_group = None;
                self.last_error = Some(err.to_string());
                cx.notify();
            }
        }
    }

    fn merge_demo(&mut self, cx: &mut Context<Self>) {
        match Edit::insert(self.cursor, "⌁".to_string()) {
            Ok(edit) => {
                self.merge_group = None;
                self.submit_edits(
                    vec![edit],
                    TransactionMetadata::new(TransactionSource::Keyboard)
                        .with_merge_policy(TransactionMergePolicy::MergeWithPrevious)
                        .with_description("合并演示"),
                    cx,
                );
            }
            Err(err) => {
                self.last_error = Some(err.to_string());
                cx.notify();
            }
        }
    }

    fn metadata_for_group(
        &self,
        group: MergeGroup,
        source: TransactionSource,
    ) -> TransactionMetadata {
        let metadata = TransactionMetadata::new(source);

        if self.merge_group == Some(group) {
            metadata.with_merge_policy(TransactionMergePolicy::MergeWithPrevious)
        } else {
            metadata
        }
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        self.merge_group = None;
        let result = self.buffer.undo();
        self.apply_history_result(result, "撤销", cx);
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        self.merge_group = None;
        let result = self.buffer.redo();
        self.apply_history_result(result, "重做", cx);
    }

    fn apply_history_result(
        &mut self,
        result: EngineResult<Option<(zom_engine::Delta, zom_engine::ChangeSet)>>,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(Some((delta, changeset))) => {
                self.last_error = None;
                self.last_delta = Some((
                    delta.old_version,
                    delta.new_version,
                    delta.edits.as_slice().len(),
                ));

                self.anchor_a = changeset.map_position(self.anchor_a);
                self.anchor_b = changeset.map_position(self.anchor_b);
                self.last_changed_ranges = changeset.changed_ranges();
                self.cursor = self.cursor_from_engine_selection();
                self.last_history_event = Some(label.to_string());
            }
            Ok(None) => {
                self.last_error = None;
                self.last_history_event = Some(format!("{label}: 历史为空"));
            }
            Err(err) => {
                self.last_error = Some(err.to_string());
            }
        }

        cx.notify();
    }

    fn move_left(&mut self, cx: &mut Context<Self>) {
        self.move_selections_horizontally(false, HorizontalDirection::Left, cx);
    }

    fn move_right(&mut self, cx: &mut Context<Self>) {
        self.move_selections_horizontally(false, HorizontalDirection::Right, cx);
    }

    fn select_left(&mut self, cx: &mut Context<Self>) {
        self.move_selections_horizontally(true, HorizontalDirection::Left, cx);
    }

    fn select_right(&mut self, cx: &mut Context<Self>) {
        self.move_selections_horizontally(true, HorizontalDirection::Right, cx);
    }

    fn move_selections_horizontally(
        &mut self,
        extend: bool,
        direction: HorizontalDirection,
        cx: &mut Context<Self>,
    ) {
        self.merge_group = None;
        let current = self.buffer.selection().clone();
        let primary_index = current.primary_index();
        let mut moved = Vec::with_capacity(current.len());

        for selection in current.as_slice() {
            let next = if extend {
                match direction {
                    HorizontalDirection::Left => {
                        self.buffer.previous_grapheme_boundary(selection.head())
                    }
                    HorizontalDirection::Right => {
                        self.buffer.next_grapheme_boundary(selection.head())
                    }
                }
                .map(|head| selection.with_head(head))
            } else if !selection.is_caret() {
                Ok(match direction {
                    HorizontalDirection::Left => selection.collapse_to_start(),
                    HorizontalDirection::Right => selection.collapse_to_end(),
                })
            } else {
                match direction {
                    HorizontalDirection::Left => self
                        .buffer
                        .previous_grapheme_boundary(selection.head())
                        .map(Selection::caret),
                    HorizontalDirection::Right => self
                        .buffer
                        .next_grapheme_boundary(selection.head())
                        .map(Selection::caret),
                }
            };

            match next {
                Ok(selection) => moved.push(selection),
                Err(err) => {
                    self.last_error = Some(err.to_string());
                    cx.notify();
                    return;
                }
            }
        }

        self.set_selection(SelectionSet::new_with_primary(moved, primary_index), cx);
    }

    fn move_to_line_start(&mut self, cx: &mut Context<Self>) {
        self.move_selections_to_line_edge(false, LineEdge::Start, cx);
    }

    fn move_to_line_end(&mut self, cx: &mut Context<Self>) {
        self.move_selections_to_line_edge(false, LineEdge::End, cx);
    }

    fn move_selections_to_line_edge(
        &mut self,
        extend: bool,
        edge: LineEdge,
        cx: &mut Context<Self>,
    ) {
        self.merge_group = None;
        let current = self.buffer.selection().clone();
        let primary_index = current.primary_index();
        let mut moved = Vec::with_capacity(current.len());

        for selection in current.as_slice() {
            let line = match self.buffer.char_to_position(selection.head()) {
                Ok(position) => position.line(),
                Err(err) => {
                    self.last_error = Some(err.to_string());
                    cx.notify();
                    return;
                }
            };

            let target = match edge {
                LineEdge::Start => self.buffer.line_start(line),
                LineEdge::End => self.line_content_end(line),
            };

            match target {
                Ok(offset) if extend => moved.push(selection.with_head(offset)),
                Ok(offset) => moved.push(Selection::caret(offset)),
                Err(err) => {
                    self.last_error = Some(err.to_string());
                    cx.notify();
                    return;
                }
            }
        }

        self.set_selection(SelectionSet::new_with_primary(moved, primary_index), cx);
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        self.merge_group = None;
        self.set_selection(
            SelectionSet::new(vec![Selection::new(
                CharOffset::ZERO,
                self.buffer.len_chars(),
            )]),
            cx,
        );
        self.last_history_event = Some("全选".to_string());
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.merge_group = None;
        self.set_selection(SelectionSet::caret(self.cursor), cx);
        self.last_history_event = Some("清除为单光标".to_string());
    }

    fn multi_cursor_demo(&mut self, cx: &mut Context<Self>) {
        self.merge_group = None;
        self.set_selection(
            SelectionSet::new(vec![
                Selection::caret(self.cursor),
                Selection::caret(self.anchor_a),
                Selection::caret(self.anchor_b),
            ]),
            cx,
        );
        self.last_history_event = Some("生成多光标：当前光标 + A + B".to_string());
    }

    fn multi_selection_demo(&mut self, cx: &mut Context<Self>) {
        self.merge_group = None;
        let a_end = self
            .anchor_a
            .checked_add(3)
            .unwrap_or(self.buffer.len_chars())
            .min(self.buffer.len_chars());
        let b_end = self
            .anchor_b
            .checked_add(3)
            .unwrap_or(self.buffer.len_chars())
            .min(self.buffer.len_chars());

        self.set_selection(
            SelectionSet::new(vec![
                Selection::new(self.anchor_a, a_end),
                Selection::new(self.anchor_b, b_end),
            ]),
            cx,
        );
        self.last_history_event = Some("选择 A/B 标记".to_string());
    }

    fn replace_selections_probe(&mut self, cx: &mut Context<Self>) {
        self.merge_group = None;
        let metadata = TransactionMetadata::new(TransactionSource::Command)
            .with_description("用 ★ 替换所有 selection");
        self.submit_selection_replacement(self.buffer.selection().clone(), "★", metadata, cx);
    }

    fn set_selection(&mut self, selection: SelectionSet, cx: &mut Context<Self>) {
        match self.buffer.set_selection(selection) {
            Ok(()) => {
                self.cursor = self.cursor_from_engine_selection();
                self.last_error = None;
            }
            Err(err) => self.last_error = Some(err.to_string()),
        }
        cx.notify();
    }

    fn line_content_end(&self, line: Line) -> EngineResult<CharOffset> {
        let line_start = self.buffer.line_start(line)?.get();
        let next_line_start = if line.get() + 1 < self.buffer.line_count() {
            self.buffer.line_start(Line::new(line.get() + 1))?.get()
        } else {
            self.buffer.len_chars().get()
        };

        Ok(CharOffset::new(line_content_end(
            self.buffer.text().as_ref(),
            line_start,
            next_line_start,
        )))
    }

    fn current_coordinate_probe(&self) -> EngineResult<CoordinateProbe> {
        let byte_offset = self.buffer.char_to_byte(self.cursor)?;
        let utf16_position = self.buffer.char_to_utf16_position(self.cursor)?;
        let char_from_byte = self.buffer.byte_to_char(byte_offset)?;
        let char_from_utf16 = self.buffer.utf16_position_to_char(utf16_position)?;
        let byte_from_utf16 = self.buffer.utf16_position_to_byte(utf16_position)?;
        let utf16_from_byte = self.buffer.byte_to_utf16_position(byte_offset)?;
        let position = self.cursor_position();
        let display_column = self.buffer.char_to_display_column(self.cursor)?;
        let next_tab_stop = self.buffer.next_tab_stop(display_column);
        let logical_from_display = self
            .buffer
            .display_to_logical_column(position.line(), display_column)?;
        let logical_from_display_previous = self.buffer.display_to_logical_column_with_affinity(
            position.line(),
            display_column,
            DisplayColumnAffinity::Previous,
        )?;
        let logical_from_display_next = self.buffer.display_to_logical_column_with_affinity(
            position.line(),
            display_column,
            DisplayColumnAffinity::Next,
        )?;
        let char_from_display = self
            .buffer
            .display_column_to_char(position.line(), display_column)?;
        let grapheme_boundary = self.buffer.is_grapheme_boundary(self.cursor)?;
        let previous_grapheme = self.buffer.previous_grapheme_boundary(self.cursor)?;
        let next_grapheme = self.buffer.next_grapheme_boundary(self.cursor)?;

        Ok(CoordinateProbe {
            char_offset: self.cursor,
            byte_offset,
            utf16_position,
            char_from_byte,
            char_from_utf16,
            byte_from_utf16,
            utf16_from_byte,
            display_column,
            next_tab_stop,
            logical_from_display,
            logical_from_display_previous,
            logical_from_display_next,
            char_from_display,
            grapheme_boundary,
            previous_grapheme,
            next_grapheme,
        })
    }

    fn coordinate_summary_label(&self) -> String {
        match self.current_coordinate_probe() {
            Ok(probe) => format!(
                "坐标：字节={} UTF-16=({}, {}) 视觉列={} 下个Tab={} 字素边界={} 前一字素={} 后一字素={}",
                probe.byte_offset.get(),
                probe.utf16_position.line().get(),
                probe.utf16_position.character().get(),
                probe.display_column.get(),
                probe.next_tab_stop.get(),
                bool_label(probe.grapheme_boundary),
                probe.previous_grapheme.get(),
                probe.next_grapheme.get(),
            ),
            Err(err) => format!("坐标错误：{err}"),
        }
    }

    fn selection_summary_label(&self) -> String {
        let selection = self.buffer.selection();
        let primary = selection.primary();
        let ranges = selection
            .as_slice()
            .iter()
            .map(|selection| {
                if selection.is_caret() {
                    format!("{}", selection.head().get())
                } else {
                    format!(
                        "{}{}{}",
                        selection.start().get(),
                        if selection.is_reversed() {
                            "←"
                        } else {
                            "→"
                        },
                        selection.end().get(),
                    )
                }
            })
            .collect::<Vec<_>>()
            .join(", ");

        format!(
            "SelectionSet 数量={} primary={} anchor={} head={} reversed={} ranges=[{}]",
            selection.len(),
            selection.primary_index(),
            primary.anchor().get(),
            primary.head().get(),
            bool_label(primary.is_reversed()),
            ranges,
        )
    }

    fn cursor_position(&self) -> Position {
        self.buffer
            .char_to_position(self.cursor)
            .unwrap_or_else(|_| Position::new(Line::ZERO, LogicalColumn::ZERO))
    }

    fn cursor_from_engine_selection(&self) -> CharOffset {
        self.buffer.selection().primary().head()
    }
}

impl Render for M6Testbed {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cursor = self.cursor;
        let position = self.cursor_position();
        let history = self.buffer.history_status();
        let delta_label = self
            .last_delta
            .map(|(old, new, edits)| format!("增量=v{}→v{} 编辑数={}", old.get(), new.get(), edits))
            .unwrap_or_else(|| "增量=无".to_string());
        let history_label = format!(
            "撤销栈={} 重做栈={} 可撤销={} 可重做={} 上次事件={}",
            history.undo_depth,
            history.redo_depth,
            bool_label(self.buffer.can_undo()),
            bool_label(self.buffer.can_redo()),
            self.last_history_event.as_deref().unwrap_or("无"),
        );
        let snapshot_label = self
            .pinned_snapshot
            .as_ref()
            .map(|snapshot| {
                format!(
                    "快照=v{} 字符={} 字节={} UTF-16={} 行数={} 状态={}",
                    snapshot.version().get(),
                    snapshot.len_chars().get(),
                    snapshot.len_bytes(),
                    snapshot.len_utf16_cu(),
                    snapshot.line_count(),
                    snapshot_state_label(self.buffer.is_snapshot_stale(snapshot)),
                )
            })
            .unwrap_or_else(|| "快照=无".to_string());
        let storage_label = format!(
            "存储=RopeyStorage 字符={} 字节={} UTF-16={} 行数={} 换行风格={}",
            self.buffer.len_chars().get(),
            self.buffer.len_bytes(),
            self.buffer.len_utf16_cu(),
            self.buffer.line_count(),
            line_ending_label(self.buffer.line_ending_style()),
        );
        let coordinate_label = self.coordinate_summary_label();
        let selection_label = self.selection_summary_label();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x18181B))
            .text_color(rgb(0xE4E4E7))
            .p_6()
            .track_focus(&self.focus_handle)
            .tab_index(0)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                let key = &event.keystroke.key;
                let modifiers = &event.keystroke.modifiers;

                let has_modifier = modifiers.platform || modifiers.control || modifiers.alt;

                if has_modifier {
                    return;
                }

                if key == "space" {
                    this.insert_text(" ", cx);
                } else if key == "tab" {
                    this.insert_text("    ", cx);
                } else if key.chars().count() == 1 {
                    this.insert_text(key, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Backspace, _window, cx| this.backspace(cx)))
            .on_action(cx.listener(|this, _: &DeleteForward, _window, cx| {
                this.delete_forward(cx)
            }))
            .on_action(cx.listener(|this, _: &Enter, _window, cx| this.insert_text("\n", cx)))
            .on_action(cx.listener(|this, _: &Left, _window, cx| this.move_left(cx)))
            .on_action(cx.listener(|this, _: &Right, _window, cx| this.move_right(cx)))
            .on_action(cx.listener(|this, _: &SelectLeft, _window, cx| this.select_left(cx)))
            .on_action(cx.listener(|this, _: &SelectRight, _window, cx| this.select_right(cx)))
            .on_action(cx.listener(|this, _: &Home, _window, cx| {
                this.move_to_line_start(cx)
            }))
            .on_action(cx.listener(|this, _: &End, _window, cx| {
                this.move_to_line_end(cx)
            }))
            .on_action(cx.listener(|this, _: &Save, _window, cx| this.mark_saved(cx)))
            .on_action(cx.listener(|this, _: &Reset, _window, cx| this.reset(cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _window, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, _: &ClearSelection, _window, cx| this.clear_selection(cx)))
            .on_action(cx.listener(|this, _: &MultiCursorDemo, _window, cx| {
                this.multi_cursor_demo(cx)
            }))
            .on_action(cx.listener(|this, _: &MultiSelectionDemo, _window, cx| {
                this.multi_selection_demo(cx)
            }))
            .on_action(cx.listener(|this, _: &ReplaceSelectionsProbe, _window, cx| {
                this.replace_selections_probe(cx)
            }))
            .on_action(cx.listener(|this, _: &BatchEdit, _window, cx| this.batch_edit(cx)))
            .on_action(cx.listener(|this, _: &Undo, _window, cx| this.undo(cx)))
            .on_action(cx.listener(|this, _: &Redo, _window, cx| this.redo(cx)))
            .on_action(cx.listener(|this, _: &MergeDemo, _window, cx| this.merge_demo(cx)))
            .on_action(cx.listener(|this, _: &CaptureSnapshot, _window, cx| {
                this.capture_snapshot(cx)
            }))
            .on_action(cx.listener(|this, _: &InsertLongLine, _window, cx| {
                this.insert_long_line(cx)
            }))
            .on_action(cx.listener(|this, _: &InsertUnicodeProbe, _window, cx| {
                this.insert_unicode_probe(cx)
            }))
            .on_action(cx.listener(|this, _: &InsertDisplayProbe, _window, cx| {
                this.insert_display_probe(cx)
            }))
            .child(
                div()
                    .border_b_1()
                    .border_color(rgb(0x3F3F46))
                    .pb_4()
                    .mb_4()
                    .child(format!(
                        "Zom Engine M6 | 光标={} / 总字符={} | 行={} 列={} | 当前版本=v{} 保存点=v{} | 修改状态={} | 锚点A={} | 锚点B={} | 变更范围={} | {} | {} | {} | {} | {} | {}",
                        cursor.get(),
                        self.buffer.len_chars().get(),
                        position.line().get(),
                        position.column().get(),
                        self.buffer.version().get(),
                        self.buffer.saved_version().get(),
                        dirty_label(self.buffer.is_dirty()),
                        self.anchor_a.get(),
                        self.anchor_b.get(),
                        self.last_changed_ranges.len(),
                        delta_label,
                        history_label,
                        snapshot_label,
                        storage_label,
                        coordinate_label,
                        selection_label,
                    )),
            )
            .child(
                div()
                    .mb_4()
                    .text_color(rgb(0xA1A1AA))
                    .child("输入字符 / 空格 / Tab / 回车；输入会替换当前所有 selection；退格 / Delete 支持多光标；← → 移动，Shift-←/→ 扩展选区；Home / End；Cmd-A 全选；Esc 清除为单光标；Cmd-J 生成多光标；Cmd-Shift-J 选择 A/B 标记；Cmd-E 用 ★ 替换所有 selection；Cmd-S 保存；Cmd-R 重置；Cmd-B 批量事务；Cmd-M 合并事务；Cmd-Z/Ctrl-Z 撤销；Cmd-Shift-Z/Ctrl-Y 重做；Cmd-K 捕获快照；Cmd-L 长行；Cmd-U Unicode/CRLF；Cmd-T 视觉列探针"),
            )
            .when_some(self.last_error.clone(), |el, error| {
                el.child(
                    div()
                        .mb_4()
                        .text_color(rgb(0xFCA5A5))
                        .child(format!("错误：{error}")),
                )
            })
            .child(render_coordinate_panel(self))
            .child(
                div()
                    .flex_1()
                    .font_family(".AppleSystemUIFont")
                    .text_xl()
                    .line_height(px(28.0))
                    .children(render_lines_with_markers(
                        self.buffer.text().as_ref(),
                        self.buffer.selection().as_slice(),
                        cursor,
                        self.anchor_a,
                        self.anchor_b,
                        &self.last_changed_ranges,
                    )),
            )
    }
}

fn render_coordinate_panel(testbed: &M6Testbed) -> Div {
    let rows: Vec<String> = match testbed.current_coordinate_probe() {
        Ok(probe) => vec![
            format!(
                "光标坐标：CharOffset={} | ByteOffset={} | UTF-16 位置=({}, {})",
                probe.char_offset.get(),
                probe.byte_offset.get(),
                probe.utf16_position.line().get(),
                probe.utf16_position.character().get(),
            ),
            format!(
                "往返转换：字节→字符={} | UTF-16→字符={} | UTF-16→字节={} | 字节→UTF-16=({}, {})",
                probe.char_from_byte.get(),
                probe.char_from_utf16.get(),
                probe.byte_from_utf16.get(),
                probe.utf16_from_byte.line().get(),
                probe.utf16_from_byte.character().get(),
            ),
            format!(
                "视觉列：当前={} | 下一个 Tab Stop={} | Tab 宽度={} | 显示策略={}",
                probe.display_column.get(),
                probe.next_tab_stop.get(),
                testbed.buffer.config().tab.tab_width(),
                display_width_policy_label(&testbed.buffer.config().display_width),
            ),
            format!(
                "视觉列→逻辑列：默认={} 向前吸附={} 向后吸附={} | 视觉列→字符={}",
                probe.logical_from_display.get(),
                probe.logical_from_display_previous.get(),
                probe.logical_from_display_next.get(),
                probe.char_from_display.get(),
            ),
            format!(
                "字素边界={} | 前一字素={} | 后一字素={} | 换行风格={}",
                bool_label(probe.grapheme_boundary),
                probe.previous_grapheme.get(),
                probe.next_grapheme.get(),
                line_ending_label(testbed.buffer.line_ending_style()),
            ),
        ],
        Err(err) => vec![format!("坐标探针错误：{err}")],
    };

    let row_elements: Vec<gpui::AnyElement> = rows
        .into_iter()
        .map(|row| div().text_color(rgb(0xD4D4D8)).child(row).into_any())
        .collect();

    div()
        .mb_4()
        .p_3()
        .border_b_1()
        .border_color(rgb(0x3F3F46))
        .bg(rgb(0x27272A))
        .child(
            div()
                .mb_2()
                .text_color(rgb(0xBFDBFE))
                .child("M6 坐标 / 视觉列 / Selection 探针"),
        )
        .children(row_elements)
}

fn initial_buffer_and_anchors() -> (Buffer, CharOffset, CharOffset) {
    let text = INITIAL_TEXT.to_string();
    let anchor_a =
        byte_to_char_offset(&text, text.find("[A]").expect("fixture should contain [A]"));
    let anchor_b =
        byte_to_char_offset(&text, text.find("[B]").expect("fixture should contain [B]"));
    let mut buffer =
        Buffer::from_text(text, BufferConfig::default()).expect("initial buffer should be valid");
    buffer
        .set_selection(SelectionSet::caret(CharOffset::ZERO))
        .expect("zero caret should be valid");

    (buffer, anchor_a, anchor_b)
}

fn byte_to_char_offset(text: &str, byte_offset: usize) -> CharOffset {
    CharOffset::new(text[..byte_offset].chars().count())
}

fn render_lines_with_markers(
    text: &str,
    selections: &[Selection],
    cursor: CharOffset,
    anchor_a: CharOffset,
    anchor_b: CharOffset,
    changed_ranges: &[TextRange],
) -> Vec<Div> {
    let mut rows = Vec::new();
    let cursor_char = cursor.get();
    let a_char = anchor_a.get();
    let b_char = anchor_b.get();

    if text.is_empty() {
        rows.push(cursor_row());
        return rows;
    }

    let mut line_start = 0;

    for line_with_newline in text.split_inclusive('\n') {
        let line_end = line_start + line_with_newline.chars().count();
        let display_line = line_with_newline
            .trim_end_matches('\n')
            .trim_end_matches('\r');

        let mut row_children: Vec<gpui::AnyElement> = Vec::new();
        let mut char_offset = line_start;

        for c in display_line.chars() {
            let next_offset = char_offset + 1;

            if has_caret_at(selections, char_offset) {
                row_children.push(cursor_element().into_any());
            }

            let mut is_highlighted = false;
            let mut is_deletion_scar = false;
            let is_selected = is_selected_char(selections, char_offset);

            for range in changed_ranges {
                let start = range.start().get();
                let end = range.end().get();

                if start == end && char_offset == start {
                    is_deletion_scar = true;
                } else if char_offset >= start && char_offset < end {
                    is_highlighted = true;
                }
            }

            let visible = match c {
                '\t' => "⇥".to_string(),
                ' ' => "·".to_string(),
                _ => c.to_string(),
            };
            let mut char_div = div().child(visible);

            if is_highlighted {
                char_div = char_div.bg(rgb(0x854D0E)).text_color(rgb(0xFEF08A));
            } else if is_selected {
                char_div = char_div.bg(rgb(0x4C1D95)).text_color(rgb(0xDDD6FE));
            } else if char_offset == a_char {
                char_div = char_div.bg(rgb(0x991B1B)).text_color(rgb(0xFECACA));
            } else if char_offset == b_char {
                char_div = char_div.bg(rgb(0x166534)).text_color(rgb(0xBBF7D0));
            }

            if is_deletion_scar {
                char_div = char_div.border_l_2().border_color(rgb(0xEF4444));
            }

            row_children.push(char_div.into_any());
            char_offset = next_offset;
        }

        if has_caret_at(selections, char_offset) {
            row_children.push(cursor_element().into_any());
        }

        for range in changed_ranges {
            if range.start().get() == range.end().get() && char_offset == range.start().get() {
                row_children.push(
                    div()
                        .h(px(22.0))
                        .border_l_2()
                        .border_color(rgb(0xEF4444))
                        .into_any(),
                );
            }
        }

        rows.push(
            div()
                .flex()
                .flex_row()
                .min_h(px(28.0))
                .children(row_children),
        );
        line_start = line_end;
    }

    if text.ends_with('\n') {
        if cursor_char == text.chars().count() {
            rows.push(cursor_row());
        } else {
            rows.push(div().min_h(px(28.0)).child(""));
        }
    }

    rows
}

fn has_caret_at(selections: &[Selection], char_offset: usize) -> bool {
    selections
        .iter()
        .any(|selection| selection.is_caret() && selection.head().get() == char_offset)
}

fn is_selected_char(selections: &[Selection], char_offset: usize) -> bool {
    selections.iter().any(|selection| {
        !selection.is_caret()
            && char_offset >= selection.start().get()
            && char_offset < selection.end().get()
    })
}

fn cursor_row() -> Div {
    div()
        .flex()
        .flex_row()
        .min_h(px(28.0))
        .child(cursor_element())
}

#[allow(dead_code)]
fn dirty_label(is_dirty: bool) -> &'static str {
    if is_dirty { "已修改" } else { "干净" }
}

#[allow(dead_code)]
fn bool_label(value: bool) -> &'static str {
    if value { "是" } else { "否" }
}

#[allow(dead_code)]
fn snapshot_state_label(is_stale: bool) -> &'static str {
    if is_stale { "已过期" } else { "有效" }
}

#[allow(dead_code)]
fn line_ending_label<T: core::fmt::Debug>(style: T) -> String {
    match format!("{style:?}").as_str() {
        "None" => "未检测".to_string(),
        "Lf" | "LF" => "LF".to_string(),
        "Crlf" | "CRLF" => "CRLF".to_string(),
        "Mixed" => "混合".to_string(),
        other => other.to_string(),
    }
}

#[allow(dead_code)]
fn display_width_policy_label<T: core::fmt::Debug>(policy: T) -> String {
    format!("{policy:?}")
        .replace("DisplayWidthPolicy", "显示宽度策略")
        .replace("cjk_width", "CJK宽度")
        .replace("emoji_width", "emoji宽度")
        .replace("ambiguous_width", "模糊宽度")
        .replace("control_width", "控制字符宽度")
        .replace("combining_mark_width", "组合标记宽度")
}

fn cursor_element() -> Div {
    div().w(px(2.0)).h(px(22.0)).bg(rgb(0x3B82F6))
}

#[allow(dead_code)]
fn map_position_after_edits(pos: CharOffset, edits: &[Edit]) -> CharOffset {
    let mut diff = 0isize;
    let pos_val = pos.get() as isize;

    for edit in edits {
        let start = edit.range.start().get() as isize;
        let end = edit.range.end().get() as isize;
        let replacement_len = edit.replacement.chars().count() as isize;

        if pos_val < start {
            break;
        }

        if pos_val < end {
            return CharOffset::new((start + diff).max(0) as usize);
        }

        diff += replacement_len - (end - start);
    }

    CharOffset::new((pos_val + diff).max(0) as usize)
}

fn line_content_end(text: &str, line_start: usize, next_line_start: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();

    if next_line_start > line_start && chars.get(next_line_start - 1) == Some(&'\n') {
        if next_line_start >= line_start + 2 && chars.get(next_line_start - 2) == Some(&'\r') {
            next_line_start - 2
        } else {
            next_line_start - 1
        }
    } else {
        next_line_start
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, None),
            KeyBinding::new("delete", DeleteForward, None),
            KeyBinding::new("enter", Enter, None),
            KeyBinding::new("left", Left, None),
            KeyBinding::new("right", Right, None),
            KeyBinding::new("shift-left", SelectLeft, None),
            KeyBinding::new("shift-right", SelectRight, None),
            KeyBinding::new("home", Home, None),
            KeyBinding::new("end", End, None),
            KeyBinding::new("cmd-s", Save, None),
            KeyBinding::new("cmd-r", Reset, None),
            KeyBinding::new("cmd-a", SelectAll, None),
            KeyBinding::new("escape", ClearSelection, None),
            KeyBinding::new("cmd-j", MultiCursorDemo, None),
            KeyBinding::new("cmd-shift-j", MultiSelectionDemo, None),
            KeyBinding::new("cmd-e", ReplaceSelectionsProbe, None),
            KeyBinding::new("cmd-b", BatchEdit, None),
            KeyBinding::new("cmd-m", MergeDemo, None),
            KeyBinding::new("cmd-k", CaptureSnapshot, None),
            KeyBinding::new("cmd-l", InsertLongLine, None),
            KeyBinding::new("cmd-u", InsertUnicodeProbe, None),
            KeyBinding::new("cmd-t", InsertDisplayProbe, None),
            KeyBinding::new("cmd-z", Undo, None),
            KeyBinding::new("ctrl-z", Undo, None),
            KeyBinding::new("cmd-shift-z", Redo, None),
            KeyBinding::new("ctrl-y", Redo, None),
        ]);

        let bounds = Bounds::centered(None, gpui::size(px(1120.0), px(820.0)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(|cx| M6Testbed::new(cx)),
        )
        .unwrap();

        cx.activate(true);
    });
}
