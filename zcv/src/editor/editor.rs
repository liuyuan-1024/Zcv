//! 统一 Editor 的跨帧状态骨架。

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Entity, EntityInputHandler, FocusHandle,
    IntoElement, Pixels, Point, Render, Styled, UTF16Selection, Window, actions, div, point,
    prelude::*, px, size,
};
use zcv_engine::{
    Buffer, BufferConfig, ByteOffset, EditOutcome, EngineResult, Motion, MovementDirection,
    MovementUnit, Selection, SelectionSet, Snapshot, TextRange, TransactionMetadata,
    TransactionSource, Utf16Offset,
};

use super::display_map::{DisplayMap, DisplayPoint, DisplayRow};
use super::element::{EditorElement, EditorInputLayout};
use super::scroll::ScrollManager;
use super::selection::SelectionHistory;
use crate::theme::{color, typography};

actions!(
    editor,
    [
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        MoveToPreviousWord,
        MoveToNextWord,
        MoveToBeginningOfLine,
        MoveToEndOfLine,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectToPreviousWord,
        SelectToNextWord,
        SelectToBeginningOfLine,
        SelectToEndOfLine,
        SelectAll,
        Backspace,
        Delete,
        Newline,
        Undo,
        Redo,
        Cut,
        Copy,
        Paste,
    ]
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EditorMode {
    SingleLine,
    AutoHeight {
        min_lines: usize,
        max_lines: Option<usize>,
    },
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EditorComposition {
    range: TextRange,
    text: Arc<str>,
    selected_range_utf16: Range<usize>,
}

impl EditorComposition {
    fn new(
        range: TextRange,
        text: impl Into<Arc<str>>,
        selected_range_utf16: Range<usize>,
    ) -> Self {
        Self {
            range,
            text: text.into(),
            selected_range_utf16,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct EditorPresentation {
    text: Arc<str>,
    marked_byte_range: Option<Range<usize>>,
    replaced_buffer_range: Option<TextRange>,
    selected_range_utf16: Option<Range<usize>>,
}

impl EditorPresentation {
    pub(super) fn new(snapshot: &Snapshot, composition: Option<&EditorComposition>) -> Self {
        let buffer_text = snapshot
            .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
            .expect("完整 Snapshot 范围必须可读取");
        let Some(composition) = composition else {
            return Self {
                text: Arc::from(buffer_text.as_str()),
                marked_byte_range: None,
                replaced_buffer_range: None,
                selected_range_utf16: None,
            };
        };

        let start = composition.range.start().get();
        let end = composition.range.end().get();
        let mut text = String::with_capacity(
            buffer_text
                .len_bytes()
                .saturating_sub(end.saturating_sub(start))
                .saturating_add(composition.text.len()),
        );
        text.push_str(&buffer_text.as_str()[..start]);
        text.push_str(&composition.text);
        text.push_str(&buffer_text.as_str()[end..]);
        let marked_byte_range = start..start + composition.text.len();
        let marked_start_utf16 = utf16_len(&text[..marked_byte_range.start]);
        let marked_len_utf16 = utf16_len(&composition.text);
        let selected_range_utf16 = Some(
            marked_start_utf16 + composition.selected_range_utf16.start.min(marked_len_utf16)
                ..marked_start_utf16 + composition.selected_range_utf16.end.min(marked_len_utf16),
        );

        Self {
            text: Arc::from(text),
            marked_byte_range: Some(marked_byte_range),
            replaced_buffer_range: Some(composition.range),
            selected_range_utf16,
        }
    }

    pub(super) fn text(&self) -> &Arc<str> {
        &self.text
    }

    pub(super) fn marked_byte_range(&self) -> Option<Range<usize>> {
        self.marked_byte_range.clone()
    }

    pub(super) fn marked_utf16_range(&self) -> Option<Range<usize>> {
        let range = self.marked_byte_range.as_ref()?;
        Some(utf16_len(&self.text[..range.start])..utf16_len(&self.text[..range.end]))
    }

    pub(super) fn selected_range_utf16(&self) -> Option<Range<usize>> {
        self.selected_range_utf16.clone()
    }

    pub(super) fn display_byte_to_buffer_byte(&self, display_byte: usize) -> ByteOffset {
        let (Some(marked), Some(replaced)) =
            (self.marked_byte_range.as_ref(), self.replaced_buffer_range)
        else {
            return ByteOffset::new(display_byte);
        };
        if display_byte <= marked.start {
            return ByteOffset::new(display_byte);
        }
        if display_byte < marked.end {
            return replaced.end();
        }
        ByteOffset::new(display_byte - marked.end + replaced.end().get())
    }

    pub(super) fn buffer_byte_to_display_byte(&self, buffer_byte: ByteOffset) -> usize {
        let (Some(marked), Some(replaced)) =
            (self.marked_byte_range.as_ref(), self.replaced_buffer_range)
        else {
            return buffer_byte.get();
        };
        if buffer_byte <= replaced.start() {
            return buffer_byte.get();
        }
        if buffer_byte < replaced.end() {
            return marked.start;
        }
        buffer_byte.get() - replaced.end().get() + marked.end
    }

    fn byte_range_from_utf16(&self, range: Range<usize>) -> Option<Range<usize>> {
        Some(
            byte_for_utf16_offset(&self.text, range.start)?
                ..byte_for_utf16_offset(&self.text, range.end)?,
        )
    }
}

pub(crate) struct Editor {
    buffer: Entity<Buffer>,
    display_map: DisplayMap,
    mode: EditorMode,
    selections: SelectionSet,
    selection_history: SelectionHistory,
    scroll_manager: ScrollManager,
    composition: Option<EditorComposition>,
    input_layout: Option<EditorInputLayout>,
    pixel_position_of_newest_cursor: Option<Point<Pixels>>,
    last_bounds: Option<Bounds<Pixels>>,
    last_line_height: Option<Pixels>,
    focus: FocusHandle,
}

impl Editor {
    pub(crate) fn single_line(cx: &mut Context<Self>) -> Self {
        let buffer = Buffer::scratch(String::new(), BufferConfig::default())
            .expect("新建空白 Buffer 不应失败");
        let buffer = cx.new(|_| buffer);
        Self::new(buffer, EditorMode::SingleLine, cx)
    }

    pub(crate) fn auto_height(
        min_lines: usize,
        max_lines: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Self {
        let buffer = Buffer::scratch(String::new(), BufferConfig::default())
            .expect("新建空白 Buffer 不应失败");
        let buffer = cx.new(|_| buffer);
        Self::new(
            buffer,
            EditorMode::AutoHeight {
                min_lines,
                max_lines,
            },
            cx,
        )
    }

    pub(crate) fn for_buffer(buffer: Entity<Buffer>, cx: &mut Context<Self>) -> Self {
        Self::new(buffer, EditorMode::Full, cx)
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(crate) fn buffer(&self) -> Entity<Buffer> {
        self.buffer.clone()
    }

    pub(crate) fn text(&self, cx: &App) -> String {
        let snapshot = self.buffer.read(cx).snapshot();
        snapshot
            .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
            .expect("完整 Editor Snapshot 范围必须可读取")
            .as_str()
            .to_owned()
    }

    pub(crate) fn is_dirty(&self, cx: &App) -> bool {
        self.buffer.read(cx).is_dirty()
    }

    pub(crate) fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.composition = None;
        let before_selections = self.selections.clone();
        let targets = SelectionSet::new(vec![Selection::new(
            ByteOffset::ZERO,
            self.buffer.read(cx).len_bytes(),
        )]);
        let text = if self.mode == EditorMode::SingleLine {
            text.replace(['\r', '\n'], "")
        } else {
            text.to_owned()
        };
        let outcome = self.buffer.update(cx, |buffer, cx| {
            let outcome = buffer.insert_at_selections(&targets, &text, edit_metadata("设置文本"));
            cx.notify();
            outcome
        });
        self.apply_edit_outcome(before_selections, outcome, cx);
    }

    pub(super) fn render_snapshot(&self) -> Snapshot {
        self.display_map.snapshot().clone()
    }

    pub(super) fn selections(&self) -> SelectionSet {
        self.selections.clone()
    }

    pub(super) fn presentation(&self) -> EditorPresentation {
        EditorPresentation::new(self.display_map.snapshot(), self.composition.as_ref())
    }

    pub(super) fn scroll_anchor(&self) -> DisplayPoint {
        self.scroll_manager.anchor()
    }

    pub(super) fn scroll_offset(&self) -> Point<Pixels> {
        self.scroll_manager.offset()
    }

    pub(super) fn longest_display_row(&self) -> DisplayRow {
        self.display_map.longest_row()
    }

    pub(super) fn set_caret(&mut self, offset: ByteOffset) {
        self.composition = None;
        self.selections = SelectionSet::caret(offset);
        self.request_autoscroll();
    }

    pub(super) fn set_input_layout(&mut self, layout: EditorInputLayout) {
        self.input_layout = Some(layout);
    }

    pub(super) fn set_ime_caret_geometry(
        &mut self,
        element_bounds: Bounds<Pixels>,
        caret_bounds: Option<Bounds<Pixels>>,
    ) {
        let Some(caret_bounds) = caret_bounds else {
            return;
        };
        self.pixel_position_of_newest_cursor = Some(point(
            caret_bounds.origin.x - element_bounds.origin.x,
            caret_bounds.origin.y - element_bounds.origin.y,
        ));
        self.last_bounds = Some(element_bounds);
        self.last_line_height = Some(caret_bounds.size.height);
    }

    pub(super) fn prepare_scroll_viewport(
        &mut self,
        viewport_size: gpui::Size<Pixels>,
        content_width: Pixels,
        line_height: Pixels,
    ) {
        self.scroll_manager.update_viewport(
            self.display_map.snapshot().line_count(),
            viewport_size.width,
            viewport_size.height,
            content_width,
            line_height,
        );
    }

    pub(super) fn scroll_by(&mut self, delta: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        if self.scroll_manager.scroll_by(delta) {
            self.input_layout = None;
            cx.notify();
            true
        } else {
            false
        }
    }

    pub(super) fn complete_autoscroll(
        &mut self,
        caret_left: Option<Pixels>,
        caret_right: Option<Pixels>,
    ) -> bool {
        self.scroll_manager
            .complete_autoscroll(caret_left, caret_right)
    }

    fn new(buffer: Entity<Buffer>, mode: EditorMode, cx: &mut Context<Self>) -> Self {
        let display_map = DisplayMap::new(buffer.read(cx).snapshot());
        cx.observe(&buffer, |editor, buffer, cx| {
            editor.display_map.set_snapshot(buffer.read(cx).snapshot());
            editor.input_layout = None;
            cx.notify();
        })
        .detach();
        Self {
            buffer,
            display_map,
            mode,
            selections: SelectionSet::default(),
            selection_history: SelectionHistory::default(),
            scroll_manager: ScrollManager::default(),
            composition: None,
            input_layout: None,
            pixel_position_of_newest_cursor: None,
            last_bounds: None,
            last_line_height: None,
            focus: cx.focus_handle(),
        }
    }

    fn selection_for_utf16_range(&self, range: Range<usize>, cx: &App) -> Option<SelectionSet> {
        let snapshot = self.buffer.read(cx).snapshot();
        let start = snapshot
            .utf16_cu_to_byte(Utf16Offset::new(range.start))
            .ok()?;
        let end = snapshot
            .utf16_cu_to_byte(Utf16Offset::new(range.end))
            .ok()?;
        Some(SelectionSet::new(vec![Selection::new(start, end)]))
    }

    fn replace_text(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        if self.composition.is_some() && text.is_empty() {
            self.composition = None;
            self.input_layout = None;
            cx.notify();
            return;
        }
        let before_selections = self.selections.clone();
        let targets = if let Some(composition) = self.composition.take() {
            SelectionSet::new(vec![Selection::new(
                composition.range.start(),
                composition.range.end(),
            )])
        } else if let Some(range) = range_utf16 {
            let Some(selection) = self.selection_for_utf16_range(range, cx) else {
                return;
            };
            selection
        } else {
            self.selections.clone()
        };
        let text = if self.mode == EditorMode::SingleLine {
            text.replace(['\r', '\n'], "")
        } else {
            text.to_owned()
        };
        let outcome = self.buffer.update(cx, |buffer, cx| {
            let outcome = buffer.insert_at_selections(&targets, &text, edit_metadata("输入文本"));
            cx.notify();
            outcome
        });
        self.apply_edit_outcome(before_selections, outcome, cx);
    }

    fn apply_edit_outcome(
        &mut self,
        before_selections: SelectionSet,
        outcome: EngineResult<EditOutcome>,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(outcome) => {
                if let Some(transaction_id) = outcome.history_transaction_id() {
                    self.selection_history.record_transaction(
                        transaction_id,
                        before_selections,
                        outcome.after_selections().clone(),
                    );
                }
                self.selections = outcome.into_after_selections();
                self.display_map
                    .set_snapshot(self.buffer.read(cx).snapshot());
                self.request_autoscroll();
                self.input_layout = None;
                cx.notify();
            }
            Err(error) => eprintln!("Editor 编辑事务失败：{error}"),
        }
    }

    fn move_selections(
        &mut self,
        direction: MovementDirection,
        motion: impl Into<Motion>,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        let outcome =
            self.buffer
                .read(cx)
                .move_selections(&self.selections, direction, motion, extend);
        match outcome {
            Ok(selections) => {
                self.composition = None;
                self.selections = selections;
                self.request_autoscroll();
                self.input_layout = None;
                cx.notify();
            }
            Err(error) => eprintln!("Editor 选区移动失败：{error}"),
        }
    }

    fn delete(
        &mut self,
        direction: MovementDirection,
        description: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.composition = None;
        let before_selections = self.selections.clone();
        let outcome = self.buffer.update(cx, |buffer, cx| {
            let outcome = buffer.delete_at_selections(
                &before_selections,
                Some((direction, MovementUnit::Grapheme)),
                edit_metadata(description),
            );
            cx.notify();
            outcome
        });
        self.apply_edit_outcome(before_selections, outcome, cx);
    }

    fn insert_newline(&mut self, cx: &mut Context<Self>) {
        if self.mode == EditorMode::SingleLine {
            cx.propagate();
            return;
        }
        self.replace_text(None, "\n", cx);
    }

    fn selected_text(&self, cx: &App) -> Option<String> {
        let snapshot = self.buffer.read(cx).snapshot();
        let mut parts = Vec::new();
        for selection in self.selections.as_slice() {
            if selection.is_caret() {
                continue;
            }
            parts.push(
                snapshot
                    .slice_text(selection.range())
                    .ok()?
                    .as_str()
                    .to_owned(),
            );
        }
        (!parts.is_empty()).then(|| parts.join("\n"))
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        let outcome = self.buffer.update(cx, |buffer, cx| {
            let outcome = buffer.undo();
            cx.notify();
            outcome
        });
        match outcome {
            Ok(Some(outcome)) => {
                if let Some(selections) = self
                    .selection_history
                    .transaction(outcome.transaction_id())
                    .map(|history| history.undo().clone())
                {
                    self.selections = selections;
                }
                self.synchronize_after_history_edit(cx);
            }
            Ok(None) => {}
            Err(error) => eprintln!("Editor Undo 失败：{error}"),
        }
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        let outcome = self.buffer.update(cx, |buffer, cx| {
            let outcome = buffer.redo();
            cx.notify();
            outcome
        });
        match outcome {
            Ok(Some(outcome)) => {
                if let Some(selections) = self
                    .selection_history
                    .transaction(outcome.transaction_id())
                    .map(|history| history.redo().clone())
                {
                    self.selections = selections;
                }
                self.synchronize_after_history_edit(cx);
            }
            Ok(None) => {}
            Err(error) => eprintln!("Editor Redo 失败：{error}"),
        }
    }

    fn synchronize_after_history_edit(&mut self, cx: &mut Context<Self>) {
        self.composition = None;
        self.display_map
            .set_snapshot(self.buffer.read(cx).snapshot());
        self.request_autoscroll();
        self.input_layout = None;
        cx.notify();
    }

    fn request_autoscroll(&mut self) {
        let head = self.selections.primary().head();
        if let Ok(point) = self.display_map.offset_to_display_point(head) {
            self.scroll_manager.request_autoscroll(point);
        }
    }

    pub(super) fn handle_move_left(
        &mut self,
        _: &MoveLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(
            MovementDirection::Previous,
            MovementUnit::Grapheme,
            false,
            cx,
        );
    }

    pub(super) fn handle_move_right(
        &mut self,
        _: &MoveRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Next, MovementUnit::Grapheme, false, cx);
    }

    pub(super) fn handle_move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        if self.mode == EditorMode::SingleLine {
            cx.propagate();
            return;
        }
        self.move_selections(MovementDirection::Previous, Motion::LineStep, false, cx);
    }

    pub(super) fn handle_move_down(
        &mut self,
        _: &MoveDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mode == EditorMode::SingleLine {
            cx.propagate();
            return;
        }
        self.move_selections(MovementDirection::Next, Motion::LineStep, false, cx);
    }

    pub(super) fn handle_move_to_previous_word(
        &mut self,
        _: &MoveToPreviousWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Previous, MovementUnit::Word, false, cx);
    }

    pub(super) fn handle_move_to_next_word(
        &mut self,
        _: &MoveToNextWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Next, MovementUnit::Word, false, cx);
    }

    pub(super) fn handle_move_to_beginning_of_line(
        &mut self,
        _: &MoveToBeginningOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(
            MovementDirection::Previous,
            MovementUnit::LineEdge,
            false,
            cx,
        );
    }

    pub(super) fn handle_move_to_end_of_line(
        &mut self,
        _: &MoveToEndOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Next, MovementUnit::LineEdge, false, cx);
    }

    pub(super) fn handle_select_left(
        &mut self,
        _: &SelectLeft,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(
            MovementDirection::Previous,
            MovementUnit::Grapheme,
            true,
            cx,
        );
    }

    pub(super) fn handle_select_right(
        &mut self,
        _: &SelectRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Next, MovementUnit::Grapheme, true, cx);
    }

    pub(super) fn handle_select_up(
        &mut self,
        _: &SelectUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Previous, Motion::LineStep, true, cx);
    }

    pub(super) fn handle_select_down(
        &mut self,
        _: &SelectDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Next, Motion::LineStep, true, cx);
    }

    pub(super) fn handle_select_to_previous_word(
        &mut self,
        _: &SelectToPreviousWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Previous, MovementUnit::Word, true, cx);
    }

    pub(super) fn handle_select_to_next_word(
        &mut self,
        _: &SelectToNextWord,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Next, MovementUnit::Word, true, cx);
    }

    pub(super) fn handle_select_to_beginning_of_line(
        &mut self,
        _: &SelectToBeginningOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(
            MovementDirection::Previous,
            MovementUnit::LineEdge,
            true,
            cx,
        );
    }

    pub(super) fn handle_select_to_end_of_line(
        &mut self,
        _: &SelectToEndOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_selections(MovementDirection::Next, MovementUnit::LineEdge, true, cx);
    }

    pub(super) fn handle_select_all(
        &mut self,
        _: &SelectAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let end = self.buffer.read(cx).len_bytes();
        self.composition = None;
        self.selections = SelectionSet::new(vec![Selection::new(ByteOffset::ZERO, end)]);
        self.request_autoscroll();
        self.input_layout = None;
        cx.notify();
    }

    pub(super) fn handle_backspace(
        &mut self,
        _: &Backspace,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete(MovementDirection::Previous, "向后删除", cx);
    }

    pub(super) fn handle_delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.delete(MovementDirection::Next, "向前删除", cx);
    }

    pub(super) fn handle_newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_newline(cx);
    }

    pub(super) fn handle_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.undo(cx);
    }

    pub(super) fn handle_redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.redo(cx);
    }

    pub(super) fn handle_copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text(cx) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(super) fn handle_cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.selected_text(cx) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.composition = None;
        let before_selections = self.selections.clone();
        let outcome = self.buffer.update(cx, |buffer, cx| {
            let outcome =
                buffer.delete_at_selections(&before_selections, None, edit_metadata("剪切"));
            cx.notify();
            outcome
        });
        self.apply_edit_outcome(before_selections, outcome, cx);
    }

    pub(super) fn handle_paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let Some(text) = item.text() else {
            return;
        };
        if !text.is_empty() {
            self.replace_text(None, &text, cx);
        }
    }
}

fn edit_metadata(description: &'static str) -> TransactionMetadata {
    TransactionMetadata::new(TransactionSource::Programmatic).with_description(description)
}

impl Render for Editor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.display_map
            .set_snapshot(self.buffer.read(cx).snapshot());

        // SingleLine / AutoHeight 用于搜索框等 UI 场景，应使用 UI 字号而非编辑器字号
        let (font, text_size, line_height) = match self.mode {
            EditorMode::SingleLine | EditorMode::AutoHeight { .. } => (
                typography::ui_font(),
                typography::ui(),
                typography::ui_line(),
            ),
            EditorMode::Full => (
                typography::editor_font(),
                typography::editor(),
                typography::editor_line(),
            ),
        };
        let visible_lines = match self.mode {
            EditorMode::SingleLine => Some(1),
            EditorMode::AutoHeight {
                min_lines,
                max_lines,
            } => {
                let line_count = self
                    .presentation()
                    .text()
                    .bytes()
                    .filter(|byte| *byte == b'\n')
                    .count()
                    .saturating_add(1)
                    .max(min_lines);
                Some(max_lines.map_or(line_count, |maximum| line_count.min(maximum)))
            }
            EditorMode::Full => None,
        };

        EditorElement::register_actions(
            div()
                .track_focus(&self.focus)
                .key_context("Editor")
                .tab_index(0)
                .cursor(CursorStyle::IBeam)
                .w_full()
                .when_some(visible_lines, |element, lines| {
                    element.h(line_height * lines)
                })
                .when(visible_lines.is_none(), |element| element.flex_1().h_full())
                .overflow_hidden()
                .font(font)
                .text_size(text_size)
                .line_height(line_height)
                .text_color(color::current().gray.s[8]),
            cx,
        )
        .child(EditorElement::new(cx.entity()))
    }
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let presentation = self.presentation();
        let range = presentation.byte_range_from_utf16(range_utf16.clone())?;
        actual_range.replace(range_utf16);
        Some(presentation.text()[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let presentation = self.presentation();
        if let Some(range) = presentation.selected_range_utf16() {
            return Some(UTF16Selection {
                range,
                reversed: false,
            });
        }

        let snapshot = self.display_map.snapshot();
        let selection = *self.selections.primary();
        Some(UTF16Selection {
            range: snapshot.byte_to_utf16_cu(selection.start()).ok()?.get()
                ..snapshot.byte_to_utf16_cu(selection.end()).ok()?.get(),
            reversed: selection.is_reversed(),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.presentation().marked_utf16_range()
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(composition) = self.composition.as_ref() else {
            return;
        };
        let text = composition.text.to_string();
        self.replace_text(None, &text, cx);
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text(range_utf16, text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = if let Some(composition) = self.composition.as_ref() {
            composition.range
        } else if let Some(range) = range_utf16 {
            let Some(selection) = self.selection_for_utf16_range(range, cx) else {
                return;
            };
            selection.primary().range()
        } else {
            self.selections.primary().range()
        };
        let text = if self.mode == EditorMode::SingleLine {
            new_text.replace(['\r', '\n'], "")
        } else {
            new_text.to_owned()
        };
        if text.is_empty() {
            self.composition = None;
            self.input_layout = None;
            cx.notify();
            return;
        }
        let text_utf16_len = utf16_len(&text);
        let selected_range = new_selected_range_utf16.unwrap_or(text_utf16_len..text_utf16_len);
        self.composition = Some(EditorComposition::new(range, text, selected_range));
        self.input_layout = None;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let cursor = self.pixel_position_of_newest_cursor?;
        let bounds = self.last_bounds?;
        Some(Bounds::new(
            point(bounds.origin.x + cursor.x, bounds.origin.y + cursor.y),
            size(px(2.), self.last_line_height?),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.input_layout.as_ref()?.utf16_index_for_point(point)
    }
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn byte_for_utf16_offset(text: &str, target: usize) -> Option<usize> {
    let mut utf16_offset = 0;
    for (byte_offset, character) in text.char_indices() {
        if utf16_offset == target {
            return Some(byte_offset);
        }
        utf16_offset += character.len_utf16();
        if utf16_offset > target {
            return None;
        }
    }
    (utf16_offset == target).then_some(text.len())
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext, Bounds, ScrollDelta, ScrollWheelEvent, TestAppContext, point, px};
    use zcv_engine::{BufferConfig, ByteOffset, DisplayColumn, SelectionSet, TransactionId};

    use super::*;
    use crate::editor::display_map::{DisplayPoint, DisplayRow};

    fn test_buffer(cx: &mut TestAppContext, text: impl Into<String>) -> Entity<Buffer> {
        let buffer =
            Buffer::scratch(text.into(), BufferConfig::default()).expect("测试 Buffer 应能创建");
        cx.new(|_| buffer)
    }

    fn buffer_text<C: AppContext>(buffer: &Entity<Buffer>, cx: &C) -> C::Result<String> {
        cx.read_entity(buffer, |buffer, _| {
            buffer
                .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
                .expect("完整测试 Buffer 应可读取")
                .as_str()
                .to_owned()
        })
    }

    #[gpui::test]
    fn editors_share_buffer_but_keep_view_state_independent(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "abc");
        let first = cx.new(|cx| Editor::for_buffer(buffer.clone(), cx));
        let second = cx.new(|cx| Editor::for_buffer(buffer.clone(), cx));

        cx.update_entity(&first, |editor, cx| {
            editor.selections = SelectionSet::caret(ByteOffset::new(1));
            editor
                .scroll_manager
                .set_anchor(DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(2)));
            editor.scroll_manager.set_offset(point(px(4.0), px(12.0)));
            editor.selection_history.record_transaction(
                TransactionId::new(1),
                SelectionSet::caret(ByteOffset::ZERO),
                editor.selections.clone(),
            );
            editor.buffer.update(cx, |buffer, cx| {
                buffer
                    .insert(ByteOffset::new(3), "d")
                    .expect("共享 Buffer 编辑应成功");
                cx.notify();
            });
        });

        cx.read_entity(&second, |editor, cx| {
            assert_eq!(editor.mode, EditorMode::Full);
            assert_eq!(editor.buffer, buffer);
            assert_eq!(editor.buffer.read(cx).len_bytes(), ByteOffset::new(4));
            assert_eq!(editor.render_snapshot().len_bytes(), ByteOffset::new(4));
            assert_eq!(editor.selections, SelectionSet::caret(ByteOffset::ZERO));
            assert_eq!(editor.scroll_manager.anchor(), DisplayPoint::ZERO);
            assert_eq!(editor.scroll_manager.offset(), point(px(0.0), px(0.0)));
            assert!(
                editor
                    .selection_history
                    .transaction(TransactionId::new(1))
                    .is_none()
            );
        });

        cx.read_entity(&first, |editor, _| {
            assert_eq!(
                editor.scroll_manager.anchor(),
                DisplayPoint::new(DisplayRow::ZERO, DisplayColumn::new(2))
            );
            let history = editor
                .selection_history
                .transaction(TransactionId::new(1))
                .expect("第一个 Editor 应保存自己的选区历史");
            assert_eq!(history.undo(), &SelectionSet::caret(ByteOffset::ZERO));
            assert_eq!(history.redo(), &SelectionSet::caret(ByteOffset::new(1)));
        });
    }

    #[gpui::test]
    fn constructors_create_expected_modes_and_independent_scratch_buffers(cx: &mut TestAppContext) {
        let single_line = cx.new(Editor::single_line);
        let auto_height = cx.new(|cx| Editor::auto_height(2, Some(6), cx));

        let single_buffer = cx.read_entity(&single_line, |editor, cx| {
            assert_eq!(editor.mode, EditorMode::SingleLine);
            assert_eq!(editor.selections, SelectionSet::default());
            assert_eq!(
                editor.display_map.version(),
                editor.buffer.read(cx).version()
            );
            let _focus = editor.focus_handle();
            editor.buffer.clone()
        });
        let auto_height_buffer = cx.read_entity(&auto_height, |editor, _| {
            assert_eq!(
                editor.mode,
                EditorMode::AutoHeight {
                    min_lines: 2,
                    max_lines: Some(6),
                }
            );
            editor.buffer.clone()
        });

        assert_ne!(single_buffer, auto_height_buffer);
    }

    #[gpui::test]
    fn editor_element_renders_multiline_unicode_text(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "a你\n😀b");
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.run_until_parked();
        cx.simulate_click(point(px(1000.), px(12.)), gpui::Modifiers::default());
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.render_snapshot().line_count(), 2);
            assert_eq!(editor.selections.primary().head(), ByteOffset::new(4));
        });
    }

    #[gpui::test]
    fn committed_input_uses_element_input_handler_and_preserves_unicode(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "");
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.simulate_click(point(px(4.), px(12.)), gpui::Modifiers::default());
        cx.simulate_input("中😀e\u{301}");

        assert_eq!(buffer_text(&buffer, cx), "中😀e\u{301}");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(
                editor.selections.primary().head(),
                ByteOffset::new("中😀e\u{301}".len())
            );
            assert!(editor.composition.is_none());
        });
    }

    #[gpui::test]
    fn editor_actions_move_extend_delete_and_restore_unicode_selection(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "a😀b");
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.simulate_click(point(px(0.), px(12.)), gpui::Modifiers::default());
        cx.dispatch_action(MoveRight);
        cx.dispatch_action(SelectRight);
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(
                editor.selections,
                SelectionSet::new(vec![Selection::new(ByteOffset::new(1), ByteOffset::new(5))])
            );
        });

        cx.dispatch_action(Backspace);
        assert_eq!(buffer_text(&buffer, cx), "ab");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.selections, SelectionSet::caret(ByteOffset::new(1)));
        });

        cx.dispatch_action(Undo);
        assert_eq!(buffer_text(&buffer, cx), "a😀b");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(
                editor.selections,
                SelectionSet::new(vec![Selection::new(ByteOffset::new(1), ByteOffset::new(5))])
            );
        });

        cx.dispatch_action(Redo);
        assert_eq!(buffer_text(&buffer, cx), "ab");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.selections, SelectionSet::caret(ByteOffset::new(1)));
        });
    }

    #[gpui::test]
    fn clipboard_actions_edit_selected_text_through_transactions(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "hello");
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.simulate_click(point(px(0.), px(12.)), gpui::Modifiers::default());
        cx.update_entity(&editor, |editor, _| {
            editor.selections =
                SelectionSet::new(vec![Selection::new(ByteOffset::new(1), ByteOffset::new(4))]);
        });
        cx.dispatch_action(Copy);
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().and_then(|item| item.text()),
                Some("ell".to_owned())
            );
        });

        cx.dispatch_action(Cut);
        assert_eq!(buffer_text(&buffer, cx), "ho");
        cx.dispatch_action(Undo);
        assert_eq!(buffer_text(&buffer, cx), "hello");

        cx.update_entity(&editor, |editor, _| {
            editor.selections = SelectionSet::caret(ByteOffset::new(5));
        });
        cx.dispatch_action(Paste);
        assert_eq!(buffer_text(&buffer, cx), "helloell");
        assert!(cx.read_entity(&buffer, |buffer, _| buffer.can_undo()));
    }

    #[gpui::test]
    fn moving_caret_beyond_viewport_scrolls_it_back_into_view(cx: &mut TestAppContext) {
        let text = (0..120)
            .map(|row| format!("line {row}\n"))
            .collect::<String>();
        let buffer = test_buffer(cx, text);
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.simulate_click(point(px(0.), px(12.)), gpui::Modifiers::default());
        for _ in 0..80 {
            cx.dispatch_action(MoveDown);
        }
        cx.run_until_parked();

        cx.read_entity(&editor, |editor, _| {
            let caret = editor.selections.primary().head();
            let caret_row = editor
                .render_snapshot()
                .byte_to_position(caret)
                .expect("caret 应保持有效")
                .line()
                .get();
            assert_eq!(caret_row, 80);
            assert!(editor.scroll_manager.anchor().row().get() > 0);
            assert!(editor.scroll_manager.anchor().row().get() <= caret_row);
        });
    }

    #[gpui::test]
    fn wheel_input_updates_editor_scroll_state(cx: &mut TestAppContext) {
        let text = (0..120)
            .map(|row| format!("line {row}\n"))
            .collect::<String>();
        let buffer = test_buffer(cx, text);
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.run_until_parked();
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(4.), px(4.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-120.))),
            ..Default::default()
        });

        cx.read_entity(&editor, |editor, _| {
            assert!(
                editor.scroll_manager.anchor().row() > DisplayRow::ZERO
                    || editor.scroll_manager.offset().y > px(0.)
            );
        });
    }

    #[gpui::test]
    fn horizontal_scroll_stops_at_content_edge_and_caret_autoscrolls(cx: &mut TestAppContext) {
        let text = "修改 zcv 模块时，请先阅读 zcv/docs/下的所有文档规范。同时查阅**[zed编辑器](https://github.com/zed-industries/zed)**的源码，看看zed是如何实现的，参考zed的实现方式，甚至是直接照搬zed的实现方式。".repeat(4);
        let buffer = test_buffer(cx, text.clone());
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.run_until_parked();
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(4.), px(4.)),
            delta: ScrollDelta::Pixels(point(px(-100_000.), px(0.))),
            ..Default::default()
        });
        let maximum = cx.read_entity(&editor, |editor, _| editor.scroll_manager.offset().x);
        assert!(maximum > px(0.));

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(4.), px(4.)),
            delta: ScrollDelta::Pixels(point(px(-100_000.), px(0.))),
            ..Default::default()
        });
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.scroll_manager.offset().x, maximum);
        });

        cx.update_entity(&editor, |editor, cx| {
            editor.scroll_manager.set_offset(point(px(0.), px(0.)));
            editor.set_caret(ByteOffset::new(text.len()));
            cx.notify();
        });
        cx.run_until_parked();
        cx.read_entity(&editor, |editor, _| {
            let scroll_left = editor.scroll_manager.offset().x;
            assert!(scroll_left > px(0.));
            assert!(scroll_left <= maximum);
            let cursor = editor
                .pixel_position_of_newest_cursor
                .expect("行尾光标应有布局位置");
            let bounds = editor.last_bounds.expect("Editor 应保存最近布局范围");
            assert!(cursor.x + px(2.) <= bounds.size.width);
        });
    }

    #[gpui::test]
    fn word_line_and_vertical_movement_use_engine_boundaries(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "alpha 你好\nxy");
        let editor = cx.new({
            let buffer = buffer.clone();
            move |cx| {
                let mut editor = Editor::for_buffer(buffer, cx);
                editor.selections = SelectionSet::caret(ByteOffset::new("alpha 你好".len()));
                editor
            }
        });

        cx.update_entity(&editor, |editor, cx| {
            editor.move_selections(MovementDirection::Previous, MovementUnit::Word, false, cx);
            assert_eq!(editor.selections.primary().head(), ByteOffset::new(6));

            editor.move_selections(MovementDirection::Next, MovementUnit::LineEdge, false, cx);
            assert_eq!(
                editor.selections.primary().head(),
                ByteOffset::new("alpha 你好".len())
            );

            editor.move_selections(MovementDirection::Next, Motion::LineStep, false, cx);
            assert_eq!(
                editor.selections.primary().head(),
                ByteOffset::new("alpha 你好\nxy".len())
            );
        });
    }

    #[gpui::test]
    fn newline_is_a_transaction_and_undo_restores_selection(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "ab");
        let editor = cx.new({
            let buffer = buffer.clone();
            move |cx| {
                let mut editor = Editor::for_buffer(buffer, cx);
                editor.selections = SelectionSet::caret(ByteOffset::new(1));
                editor
            }
        });

        cx.update_entity(&editor, |editor, cx| editor.insert_newline(cx));
        assert_eq!(buffer_text(&buffer, cx), "a\nb");
        assert!(cx.read_entity(&buffer, |buffer, _| buffer.can_undo()));
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.selections, SelectionSet::caret(ByteOffset::new(2)));
        });

        cx.update_entity(&editor, |editor, cx| editor.undo(cx));
        assert_eq!(buffer_text(&buffer, cx), "ab");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.selections, SelectionSet::caret(ByteOffset::new(1)));
        });
    }

    #[gpui::test]
    fn marked_text_stays_out_of_buffer_and_unmark_commits_it(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "ab");
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| {
                let mut editor = Editor::for_buffer(buffer, cx);
                editor.selections = SelectionSet::caret(ByteOffset::new(1));
                editor
            }
        });

        cx.update(|window, app| {
            editor.update(app, |editor, cx| {
                editor.replace_and_mark_text_in_range(None, "中文😀", Some(2..2), window, cx);
            });
        });
        cx.refresh().expect("测试窗口应可刷新");
        cx.run_until_parked();

        assert_eq!(buffer_text(&buffer, cx), "ab");
        cx.update(|window, app| {
            editor.update(app, |editor, cx| {
                let marked = editor
                    .marked_text_range(window, cx)
                    .expect("应存在 marked range");
                let selected = editor
                    .selected_text_range(false, window, cx)
                    .expect("应存在 composition 相对选区");
                assert_eq!(marked, 1..5);
                assert_eq!(selected.range, 3..3);
                assert!(
                    editor
                        .bounds_for_range(marked.end..marked.end, Bounds::default(), window, cx)
                        .is_some()
                );
                editor.unmark_text(window, cx);
            });
        });

        assert_eq!(buffer_text(&buffer, cx), "a中文😀b");
        cx.read_entity(&editor, |editor, _| {
            assert!(editor.composition.is_none());
            assert_eq!(editor.selections.primary().head(), ByteOffset::new(11));
        });
    }

    #[gpui::test]
    fn marked_text_can_cancel_and_committed_range_uses_utf16_offsets(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "a😀b");
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.update(|window, app| {
            editor.update(app, |editor, cx| {
                editor.replace_and_mark_text_in_range(None, "候选", None, window, cx);
                editor.replace_text_in_range(None, "", window, cx);
                assert!(editor.composition.is_none());
                editor.replace_text_in_range(Some(1..3), "你", window, cx);
            });
        });

        assert_eq!(buffer_text(&buffer, cx), "a你b");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.selections.primary().head(), ByteOffset::new(4));
        });
    }

    #[gpui::test]
    fn ime_candidate_bounds_survive_composition_and_scroll_layout_invalidation(
        cx: &mut TestAppContext,
    ) {
        let text = (0..40)
            .map(|row| format!("line {row}\n"))
            .collect::<String>();
        let buffer = test_buffer(cx, text);
        let (editor, cx) = cx.add_window_view({
            let buffer = buffer.clone();
            move |_, cx| Editor::for_buffer(buffer, cx)
        });
        let element_bounds = Bounds::new(point(px(100.), px(200.)), size(px(500.), px(300.)));
        let caret_bounds = Bounds::new(point(px(124.), px(260.)), size(px(2.), px(20.)));

        cx.update(|window, app| {
            editor.update(app, |editor, cx| {
                editor.set_ime_caret_geometry(element_bounds, Some(caret_bounds));
                editor.replace_and_mark_text_in_range(None, "中文", Some(2..2), window, cx);
                assert!(editor.input_layout.is_none());
                assert_eq!(
                    editor.bounds_for_range(2..2, element_bounds, window, cx),
                    Some(caret_bounds)
                );

                editor.prepare_scroll_viewport(size(px(100.), px(100.)), px(200.), px(20.));
                assert!(editor.scroll_by(point(px(0.), px(-60.)), cx));
                assert_eq!(
                    editor.bounds_for_range(2..2, element_bounds, window, cx),
                    Some(caret_bounds)
                );
            });
        });
    }
}
