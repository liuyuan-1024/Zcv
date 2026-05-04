//! M6B GPUI testbed：Word / Identifier / Subword / Symbol 移动语义。
//!
//! 这个 example 是“人类体感 / UI 桥接”验证入口，不替代 `tests/m6b_word_movement.rs`。
//! 重点：确认 GPUI 快捷键能正确调用 M6B public API，并且不会丢掉 M6A 的多光标、
//! 多选区、Undo / Redo、Save / Reset 等基本手感。

use gpui::{
    App, Application, Bounds, Context, FocusHandle, Focusable, IntoElement, KeyBinding,
    KeyDownEvent, Render, Window, WindowBounds, WindowOptions, actions, black, div, prelude::*, px,
    rgb, size, white,
};
use zom_engine::{
    Buffer, BufferConfig, CharOffset, DisplayColumn, EngineResult, MovementDirection, MovementUnit,
    Position, Selection, SelectionSet,
};

actions!(
    m6b_testbed,
    [
        MoveLeft,
        MoveRight,
        SelectLeft,
        SelectRight,
        MoveWordLeft,
        MoveWordRight,
        SelectWordLeft,
        SelectWordRight,
        MoveIdentifierLeft,
        MoveIdentifierRight,
        SelectIdentifierLeft,
        SelectIdentifierRight,
        MoveSubwordLeft,
        MoveSubwordRight,
        SelectSubwordLeft,
        SelectSubwordRight,
        MoveSymbolLeft,
        MoveSymbolRight,
        SelectSymbolLeft,
        SelectSymbolRight,
        InsertSpace,
        InsertTab,
        InsertNewline,
        Backspace,
        Delete,
        Undo,
        Redo,
        Save,
        Reset,
        DemoMultiCursor,
        CollapseToPrimary,
        SelectAll,
        UseGraphemeMode,
        UseWordMode,
        UseIdentifierMode,
        UseSubwordMode,
        UseSymbolMode,
        MoveActiveUnitLeft,
        MoveActiveUnitRight,
        SelectActiveUnitLeft,
        SelectActiveUnitRight,
        Quit,
    ]
);

const SAMPLE_TEXT: &str = "M6B word movement testbed\n\n\
Unicode words: hello 世界 café naïve 東京 かな カナ 한글\n\
Identifiers: user_id $value snake_case HTTPServer parseJSON2\n\
Subwords: parseHTTPResponse userID2 PascalCase camelCase snake_case\n\
Symbols: a+b == c && value != null :: path.to.method -> result\n\n\
Try: Alt-←/→ word, Ctrl-←/→ identifier, Cmd-←/→ subword, Cmd-Alt-←/→ symbol.\n\
Shift extends selection. Cmd-M creates demo multi-cursors. Type to replace all selections.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LastOperationKind {
    Info,
    Edit,
    Move,
    History,
    Error,
}

impl LastOperationKind {
    fn label(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Edit => "edit",
            Self::Move => "move",
            Self::History => "history",
            Self::Error => "error",
        }
    }
}

struct M6bTestbed {
    buffer: Buffer,
    focus_handle: FocusHandle,
    active_unit: MovementUnit,
    last_operation: LastOperationKind,
    message: String,
    saved_label: String,
}

impl M6bTestbed {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut buffer = Buffer::from_text(SAMPLE_TEXT.to_string(), BufferConfig::default())
            .expect("M6B testbed sample text should be valid");
        buffer.mark_saved();

        let mut this = Self {
            buffer,
            focus_handle: cx.focus_handle(),
            active_unit: MovementUnit::Word,
            last_operation: LastOperationKind::Info,
            message: "M6B ready: Word / Identifier / Subword / Symbol movement".to_string(),
            saved_label: "saved at initial version".to_string(),
        };
        this.place_initial_cursor();
        this
    }

    fn place_initial_cursor(&mut self) {
        let offset = find_char_offset(self.buffer.text().as_ref(), "parseHTTPResponse")
            .unwrap_or(CharOffset::ZERO);
        let _ = self.buffer.set_selection(SelectionSet::caret(offset));
    }

    fn set_message(
        &mut self,
        kind: LastOperationKind,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.last_operation = kind;
        self.message = message.into();
        cx.notify();
    }

    fn handle_result<T>(
        &mut self,
        result: EngineResult<T>,
        ok_kind: LastOperationKind,
        ok_message: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> Option<T> {
        match result {
            Ok(value) => {
                self.set_message(ok_kind, ok_message, cx);
                Some(value)
            }
            Err(error) => {
                self.set_message(LastOperationKind::Error, format!("{error:?}"), cx);
                None
            }
        }
    }

    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let selections = self.buffer.selection().clone();
        let count = selections.len();
        let result = self.buffer.insert_at_selections(selections, text);
        self.handle_result(
            result,
            LastOperationKind::Edit,
            format!("insert {:?} at {count} selection(s)", text),
            cx,
        );
    }

    fn delete_backward(&mut self, cx: &mut Context<Self>) {
        let selections = self.buffer.selection().clone();
        let result = self.buffer.delete_backward_at_selections(selections);
        self.handle_result(result, LastOperationKind::Edit, "backspace", cx);
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
        let selections = self.buffer.selection().clone();
        let result = self.buffer.delete_forward_at_selections(selections);
        self.handle_result(result, LastOperationKind::Edit, "delete", cx);
    }

    fn move_current(
        &mut self,
        direction: MovementDirection,
        unit: MovementUnit,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        // 普通左右键在非扩展移动时更接近真实编辑器：非空选区先塌缩到起点/终点。
        if unit == MovementUnit::Grapheme && !extend {
            let current = self.buffer.selection().clone();
            if current
                .as_slice()
                .iter()
                .any(|selection| !selection.is_caret())
            {
                let primary_index = current.primary_index();
                let collapsed: Vec<Selection> = current
                    .as_slice()
                    .iter()
                    .copied()
                    .map(|selection| match direction {
                        MovementDirection::Previous => selection.collapse_to_start(),
                        MovementDirection::Next => selection.collapse_to_end(),
                    })
                    .collect();
                let result = self
                    .buffer
                    .set_selection(SelectionSet::new_with_primary(collapsed, primary_index));
                self.handle_result(result, LastOperationKind::Move, "collapse selection", cx);
                return;
            }
        }

        let result = self.buffer.move_current_selection(direction, unit, extend);
        let msg = format!(
            "{} {} {}",
            if extend { "select" } else { "move" },
            unit_label(unit),
            direction_label(direction),
        );
        self.handle_result(result, LastOperationKind::Move, msg, cx);
    }

    fn use_unit(&mut self, unit: MovementUnit, cx: &mut Context<Self>) {
        self.active_unit = unit;
        self.set_message(
            LastOperationKind::Info,
            format!("active movement unit = {}", unit_label(unit)),
            cx,
        );
    }

    fn reset_buffer(&mut self, cx: &mut Context<Self>) {
        match Buffer::from_text(SAMPLE_TEXT.to_string(), BufferConfig::default()) {
            Ok(mut buffer) => {
                buffer.mark_saved();
                self.buffer = buffer;
                self.saved_label = "saved at reset version".to_string();
                self.place_initial_cursor();
                self.set_message(LastOperationKind::Info, "reset sample text", cx);
            }
            Err(error) => self.set_message(LastOperationKind::Error, format!("{error:?}"), cx),
        }
    }

    fn mark_saved(&mut self, cx: &mut Context<Self>) {
        self.buffer.mark_saved();
        self.saved_label = format!("saved at v{}", self.buffer.version().get());
        self.set_message(LastOperationKind::Info, "mark saved", cx);
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        match self.buffer.undo() {
            Ok(Some(_)) => self.set_message(LastOperationKind::History, "undo", cx),
            Ok(None) => self.set_message(LastOperationKind::History, "nothing to undo", cx),
            Err(error) => self.set_message(LastOperationKind::Error, format!("{error:?}"), cx),
        }
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        match self.buffer.redo() {
            Ok(Some(_)) => self.set_message(LastOperationKind::History, "redo", cx),
            Ok(None) => self.set_message(LastOperationKind::History, "nothing to redo", cx),
            Err(error) => self.set_message(LastOperationKind::Error, format!("{error:?}"), cx),
        }
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        let end = self.buffer.len_chars();
        let selection = SelectionSet::new(vec![Selection::new(CharOffset::ZERO, end)]);
        let result = self.buffer.set_selection(selection);
        self.handle_result(result, LastOperationKind::Move, "select all", cx);
    }

    fn collapse_to_primary(&mut self, cx: &mut Context<Self>) {
        let head = self.buffer.selection().primary().head();
        let result = self.buffer.set_selection(SelectionSet::caret(head));
        self.handle_result(
            result,
            LastOperationKind::Move,
            "collapse to primary cursor",
            cx,
        );
    }

    fn demo_multi_cursor(&mut self, cx: &mut Context<Self>) {
        let text = self.buffer.text();
        let text = text.as_ref();
        let mut selections = Vec::new();

        for needle in ["hello", "user_id", "parseHTTPResponse", "a+b", "世界"] {
            if let Some(offset) = find_char_offset(text, needle) {
                selections.push(Selection::caret(offset));
            }
        }

        if selections.is_empty() {
            self.set_message(
                LastOperationKind::Error,
                "demo cursor anchors not found",
                cx,
            );
            return;
        }

        let count = selections.len();
        let result = self.buffer.set_selection(SelectionSet::new(selections));
        self.handle_result(
            result,
            LastOperationKind::Move,
            format!("created {count} demo cursors"),
            cx,
        );
    }

    fn primary_status(&self) -> String {
        let selection = self.buffer.selection().primary();
        let head = selection.head();
        let position = self.buffer.char_to_position(head).unwrap_or(Position::ZERO);
        let display_column = self
            .buffer
            .char_to_display_column(head)
            .unwrap_or(DisplayColumn::ZERO);
        let utf16 = self
            .buffer
            .char_to_utf16_position(head)
            .ok()
            .map(|position| {
                format!(
                    "utf16=({}, {})",
                    position.line().get(),
                    position.character().get()
                )
            })
            .unwrap_or_else(|| "utf16=<invalid>".to_string());

        format!(
            "head={} | line={} col={} display={} | {}",
            head.get(),
            position.line().get(),
            position.column().get(),
            display_column.get(),
            utf16
        )
    }

    fn boundary_preview(&self) -> String {
        let text = self.buffer.text();
        let text = text.as_ref();
        let head = self.buffer.selection().primary().head();
        let previous = self
            .buffer
            .previous_movement_boundary(head, self.active_unit)
            .unwrap_or(head);
        let next = self
            .buffer
            .next_movement_boundary(head, self.active_unit)
            .unwrap_or(head);
        let previous_text = slice_chars(text, previous, head).replace('\n', "⏎");
        let next_text = slice_chars(text, head, next).replace('\n', "⏎");

        format!(
            "active={} | previous {}..{} = {:?} | next {}..{} = {:?}",
            unit_label(self.active_unit),
            previous.get(),
            head.get(),
            previous_text,
            head.get(),
            next.get(),
            next_text
        )
    }

    fn status_lines(&self) -> Vec<String> {
        let history = self.buffer.history_status();
        vec![
            format!(
                "M6B GPUI testbed | {} | dirty={} | v{} | saved={} | lines={} chars={} bytes={}",
                self.last_operation.label(),
                self.buffer.is_dirty(),
                self.buffer.version().get(),
                self.saved_label,
                self.buffer.line_count(),
                self.buffer.len_chars().get(),
                self.buffer.len_bytes(),
            ),
            format!(
                "selection count={} primary={} | undo={} redo={}",
                self.buffer.selection().len(),
                self.buffer.selection().primary_index(),
                history.undo_depth,
                history.redo_depth,
            ),
            self.primary_status(),
            self.boundary_preview(),
            format!("last: {}", self.message),
        ]
    }

    fn help_lines(&self) -> Vec<&'static str> {
        vec![
            "Input: type normal chars; Space / Tab / Enter; Backspace / Delete",
            "Basic movement: ← / →, Shift-← / Shift-→, Cmd-A select all, Esc collapse to primary",
            "M6B movement: Alt-←/→ Word, Ctrl-←/→ Identifier, Cmd-←/→ Subword, Cmd-Alt-←/→ Symbol",
            "M6B extend: add Shift to the movement shortcut to extend selection",
            "Active unit: Cmd-1 Grapheme, Cmd-2 Word, Cmd-3 Identifier, Cmd-4 Subword, Cmd-5 Symbol",
            "Active movement: Ctrl-Alt-←/→ move by active unit; Ctrl-Alt-Shift-←/→ extend by active unit",
            "Multi-cursor demo: Cmd-M creates representative cursors; typing replaces all selections",
            "History/lifecycle: Cmd-Z undo, Cmd-Shift-Z redo, Cmd-S mark saved, Cmd-R reset, Cmd-Q quit",
        ]
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(key_char) = event.keystroke.key_char.as_ref() else {
            return;
        };

        if key_char.chars().any(|ch| ch.is_control()) {
            return;
        }

        // 快捷键已经由 GPUI action dispatch 处理；这里主要覆盖普通字符输入。
        // 如果某个平台把带修饰键的快捷键也暴露为 key_char，最多只影响 testbed，
        // 不影响 engine 契约测试。
        if key_char != " " {
            self.insert_text(key_char, cx);
        }
    }

    fn move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(
            MovementDirection::Previous,
            MovementUnit::Grapheme,
            false,
            cx,
        );
    }

    fn move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Next, MovementUnit::Grapheme, false, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(
            MovementDirection::Previous,
            MovementUnit::Grapheme,
            true,
            cx,
        );
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Next, MovementUnit::Grapheme, true, cx);
    }

    fn move_word_left(&mut self, _: &MoveWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Previous, MovementUnit::Word, false, cx);
    }

    fn move_word_right(&mut self, _: &MoveWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Next, MovementUnit::Word, false, cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Previous, MovementUnit::Word, true, cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Next, MovementUnit::Word, true, cx);
    }

    fn move_identifier_left(
        &mut self,
        _: &MoveIdentifierLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(
            MovementDirection::Previous,
            MovementUnit::Identifier,
            false,
            cx,
        );
    }

    fn move_identifier_right(
        &mut self,
        _: &MoveIdentifierRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Next, MovementUnit::Identifier, false, cx);
    }

    fn select_identifier_left(
        &mut self,
        _: &SelectIdentifierLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(
            MovementDirection::Previous,
            MovementUnit::Identifier,
            true,
            cx,
        );
    }

    fn select_identifier_right(
        &mut self,
        _: &SelectIdentifierRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Next, MovementUnit::Identifier, true, cx);
    }

    fn move_subword_left(&mut self, _: &MoveSubwordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(
            MovementDirection::Previous,
            MovementUnit::Subword,
            false,
            cx,
        );
    }

    fn move_subword_right(&mut self, _: &MoveSubwordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Next, MovementUnit::Subword, false, cx);
    }

    fn select_subword_left(
        &mut self,
        _: &SelectSubwordLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Previous, MovementUnit::Subword, true, cx);
    }

    fn select_subword_right(
        &mut self,
        _: &SelectSubwordRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Next, MovementUnit::Subword, true, cx);
    }

    fn move_symbol_left(&mut self, _: &MoveSymbolLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Previous, MovementUnit::Symbol, false, cx);
    }

    fn move_symbol_right(&mut self, _: &MoveSymbolRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Next, MovementUnit::Symbol, false, cx);
    }

    fn select_symbol_left(&mut self, _: &SelectSymbolLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_current(MovementDirection::Previous, MovementUnit::Symbol, true, cx);
    }

    fn select_symbol_right(
        &mut self,
        _: &SelectSymbolRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Next, MovementUnit::Symbol, true, cx);
    }

    fn insert_space(&mut self, _: &InsertSpace, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_text(" ", cx);
    }

    fn insert_tab(&mut self, _: &InsertTab, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_text("\t", cx);
    }

    fn insert_newline(&mut self, _: &InsertNewline, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_text("\n", cx);
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_backward(cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_forward(cx);
    }

    fn undo_action(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.undo(cx);
    }

    fn redo_action(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.redo(cx);
    }

    fn save_action(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        self.mark_saved(cx);
    }

    fn reset_action(&mut self, _: &Reset, _: &mut Window, cx: &mut Context<Self>) {
        self.reset_buffer(cx);
    }

    fn demo_multi_cursor_action(
        &mut self,
        _: &DemoMultiCursor,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.demo_multi_cursor(cx);
    }

    fn collapse_to_primary_action(
        &mut self,
        _: &CollapseToPrimary,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.collapse_to_primary(cx);
    }

    fn select_all_action(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.select_all(cx);
    }

    fn use_grapheme_mode(&mut self, _: &UseGraphemeMode, _: &mut Window, cx: &mut Context<Self>) {
        self.use_unit(MovementUnit::Grapheme, cx);
    }

    fn use_word_mode(&mut self, _: &UseWordMode, _: &mut Window, cx: &mut Context<Self>) {
        self.use_unit(MovementUnit::Word, cx);
    }

    fn use_identifier_mode(
        &mut self,
        _: &UseIdentifierMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.use_unit(MovementUnit::Identifier, cx);
    }

    fn use_subword_mode(&mut self, _: &UseSubwordMode, _: &mut Window, cx: &mut Context<Self>) {
        self.use_unit(MovementUnit::Subword, cx);
    }

    fn use_symbol_mode(&mut self, _: &UseSymbolMode, _: &mut Window, cx: &mut Context<Self>) {
        self.use_unit(MovementUnit::Symbol, cx);
    }

    fn move_active_unit_left(
        &mut self,
        _: &MoveActiveUnitLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Previous, self.active_unit, false, cx);
    }

    fn move_active_unit_right(
        &mut self,
        _: &MoveActiveUnitRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Next, self.active_unit, false, cx);
    }

    fn select_active_unit_left(
        &mut self,
        _: &SelectActiveUnitLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Previous, self.active_unit, true, cx);
    }

    fn select_active_unit_right(
        &mut self,
        _: &SelectActiveUnitRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_current(MovementDirection::Next, self.active_unit, true, cx);
    }
}

impl Focusable for M6bTestbed {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for M6bTestbed {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let decorated_text = decorate_text(self.buffer.text().as_ref(), self.buffer.selection());
        let status_lines = self.status_lines();
        let help_lines = self.help_lines();

        div()
            .key_context("M6bTestbed")
            .track_focus(&self.focus_handle(cx))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_action(cx.listener(Self::move_left))
            .on_action(cx.listener(Self::move_right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::move_word_left))
            .on_action(cx.listener(Self::move_word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::move_identifier_left))
            .on_action(cx.listener(Self::move_identifier_right))
            .on_action(cx.listener(Self::select_identifier_left))
            .on_action(cx.listener(Self::select_identifier_right))
            .on_action(cx.listener(Self::move_subword_left))
            .on_action(cx.listener(Self::move_subword_right))
            .on_action(cx.listener(Self::select_subword_left))
            .on_action(cx.listener(Self::select_subword_right))
            .on_action(cx.listener(Self::move_symbol_left))
            .on_action(cx.listener(Self::move_symbol_right))
            .on_action(cx.listener(Self::select_symbol_left))
            .on_action(cx.listener(Self::select_symbol_right))
            .on_action(cx.listener(Self::insert_space))
            .on_action(cx.listener(Self::insert_tab))
            .on_action(cx.listener(Self::insert_newline))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::undo_action))
            .on_action(cx.listener(Self::redo_action))
            .on_action(cx.listener(Self::save_action))
            .on_action(cx.listener(Self::reset_action))
            .on_action(cx.listener(Self::demo_multi_cursor_action))
            .on_action(cx.listener(Self::collapse_to_primary_action))
            .on_action(cx.listener(Self::select_all_action))
            .on_action(cx.listener(Self::use_grapheme_mode))
            .on_action(cx.listener(Self::use_word_mode))
            .on_action(cx.listener(Self::use_identifier_mode))
            .on_action(cx.listener(Self::use_subword_mode))
            .on_action(cx.listener(Self::use_symbol_mode))
            .on_action(cx.listener(Self::move_active_unit_left))
            .on_action(cx.listener(Self::move_active_unit_right))
            .on_action(cx.listener(Self::select_active_unit_left))
            .on_action(cx.listener(Self::select_active_unit_right))
            .size_full()
            .flex()
            .flex_col()
            .gap_3()
            .bg(rgb(0x1f2328))
            .text_color(white())
            .p(px(16.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .border_1()
                    .border_color(rgb(0x6e7681))
                    .bg(rgb(0x0d1117))
                    .p(px(12.0))
                    .children(status_lines.into_iter()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .border_1()
                    .border_color(rgb(0x6e7681))
                    .bg(rgb(0x161b22))
                    .p(px(12.0))
                    .text_size(px(14.0))
                    .line_height(px(22.0))
                    .children(help_lines.into_iter()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(black())
                    .bg(white())
                    .text_color(black())
                    .p(px(12.0))
                    .text_size(px(18.0))
                    .line_height(px(28.0))
                    .child(decorated_text),
            )
    }
}

fn unit_label(unit: MovementUnit) -> &'static str {
    match unit {
        MovementUnit::Grapheme => "grapheme",
        MovementUnit::Word => "word",
        MovementUnit::Identifier => "identifier",
        MovementUnit::Subword => "subword",
        MovementUnit::Symbol => "symbol",
    }
}

fn direction_label(direction: MovementDirection) -> &'static str {
    match direction {
        MovementDirection::Previous => "left",
        MovementDirection::Next => "right",
    }
}

fn find_char_offset(text: &str, needle: &str) -> Option<CharOffset> {
    let byte = text.find(needle)?;
    Some(CharOffset::new(text[..byte].chars().count()))
}

fn slice_chars(text: &str, start: CharOffset, end: CharOffset) -> String {
    let start = start.get().min(text.chars().count());
    let end = end.get().min(text.chars().count()).max(start);
    text.chars().skip(start).take(end - start).collect()
}

fn decorate_text(text: &str, selections: &SelectionSet) -> String {
    let len = text.chars().count();
    let mut carets = vec![0usize; len + 1];
    let mut opens = vec![0usize; len + 1];
    let mut closes = vec![0usize; len + 1];

    for selection in selections.as_slice() {
        if selection.is_caret() {
            carets[selection.head().get().min(len)] += 1;
        } else {
            opens[selection.start().get().min(len)] += 1;
            closes[selection.end().get().min(len)] += 1;
        }
    }

    let mut out = String::new();
    out.push_str("Legend: ┃ caret, ⟦selection⟧ range. Offsets are CharOffset.\n\n");

    for index in 0..=len {
        for _ in 0..closes[index] {
            out.push('⟧');
        }
        for _ in 0..opens[index] {
            out.push('⟦');
        }
        if carets[index] == 1 {
            out.push('┃');
        } else if carets[index] > 1 {
            out.push_str(&format!("┃×{}", carets[index]));
        }

        if index < len {
            if let Some(ch) = text.chars().nth(index) {
                out.push(ch);
            }
        }
    }

    out
}

fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("left", MoveLeft, None),
        KeyBinding::new("right", MoveRight, None),
        KeyBinding::new("shift-left", SelectLeft, None),
        KeyBinding::new("shift-right", SelectRight, None),
        KeyBinding::new("alt-left", MoveWordLeft, None),
        KeyBinding::new("alt-right", MoveWordRight, None),
        KeyBinding::new("alt-shift-left", SelectWordLeft, None),
        KeyBinding::new("alt-shift-right", SelectWordRight, None),
        KeyBinding::new("ctrl-left", MoveIdentifierLeft, None),
        KeyBinding::new("ctrl-right", MoveIdentifierRight, None),
        KeyBinding::new("ctrl-shift-left", SelectIdentifierLeft, None),
        KeyBinding::new("ctrl-shift-right", SelectIdentifierRight, None),
        KeyBinding::new("cmd-left", MoveSubwordLeft, None),
        KeyBinding::new("cmd-right", MoveSubwordRight, None),
        KeyBinding::new("cmd-shift-left", SelectSubwordLeft, None),
        KeyBinding::new("cmd-shift-right", SelectSubwordRight, None),
        KeyBinding::new("cmd-alt-left", MoveSymbolLeft, None),
        KeyBinding::new("cmd-alt-right", MoveSymbolRight, None),
        KeyBinding::new("cmd-alt-shift-left", SelectSymbolLeft, None),
        KeyBinding::new("cmd-alt-shift-right", SelectSymbolRight, None),
        KeyBinding::new("space", InsertSpace, None),
        KeyBinding::new("tab", InsertTab, None),
        KeyBinding::new("enter", InsertNewline, None),
        KeyBinding::new("backspace", Backspace, None),
        KeyBinding::new("delete", Delete, None),
        KeyBinding::new("cmd-z", Undo, None),
        KeyBinding::new("cmd-shift-z", Redo, None),
        KeyBinding::new("ctrl-y", Redo, None),
        KeyBinding::new("cmd-s", Save, None),
        KeyBinding::new("cmd-r", Reset, None),
        KeyBinding::new("cmd-m", DemoMultiCursor, None),
        KeyBinding::new("escape", CollapseToPrimary, None),
        KeyBinding::new("cmd-a", SelectAll, None),
        KeyBinding::new("cmd-1", UseGraphemeMode, None),
        KeyBinding::new("cmd-2", UseWordMode, None),
        KeyBinding::new("cmd-3", UseIdentifierMode, None),
        KeyBinding::new("cmd-4", UseSubwordMode, None),
        KeyBinding::new("cmd-5", UseSymbolMode, None),
        KeyBinding::new("ctrl-alt-left", MoveActiveUnitLeft, None),
        KeyBinding::new("ctrl-alt-right", MoveActiveUnitRight, None),
        KeyBinding::new("ctrl-alt-shift-left", SelectActiveUnitLeft, None),
        KeyBinding::new("ctrl-alt-shift-right", SelectActiveUnitRight, None),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
}

fn main() {
    Application::new().run(|cx: &mut App| {
        bind_keys(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());

        let bounds = Bounds::centered(None, size(px(1180.0), px(820.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| cx.new(M6bTestbed::new),
            )
            .expect("open M6B testbed window");

        window
            .update(cx, |view, window, cx| {
                window.focus(&view.focus_handle(cx));
                cx.activate(true);
            })
            .expect("focus M6B testbed window");
    });
}
