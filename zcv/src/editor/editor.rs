//! 统一 Editor 的跨帧状态骨架。

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Bounds, Context, CursorStyle, EntityInputHandler, FocusHandle, IntoElement, Pixels, Point,
    Render, Styled, UTF16Selection, Window, div, prelude::*,
};
use zcv_engine::{
    Buffer, BufferConfig, ByteOffset, Selection, SelectionSet, Snapshot, TextRange,
    TransactionMetadata, TransactionSource, Utf16Offset,
};

use super::display_map::{DisplayMap, DisplayPoint};
use super::element::{EditorElement, EditorInputLayout};
use super::scroll::ScrollManager;
use super::selection::SelectionHistory;
use crate::theme::{color, typography};

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
    buffer: Rc<RefCell<Buffer>>,
    display_map: DisplayMap,
    mode: EditorMode,
    selections: SelectionSet,
    selection_history: SelectionHistory,
    scroll_manager: ScrollManager,
    composition: Option<EditorComposition>,
    input_layout: Option<EditorInputLayout>,
    focus: FocusHandle,
}

impl Editor {
    pub(crate) fn single_line(cx: &mut Context<Self>) -> Self {
        let buffer = Buffer::scratch(String::new(), BufferConfig::default())
            .expect("新建空白 Buffer 不应失败");
        Self::new(Rc::new(RefCell::new(buffer)), EditorMode::SingleLine, cx)
    }

    pub(crate) fn auto_height(
        min_lines: usize,
        max_lines: Option<usize>,
        cx: &mut Context<Self>,
    ) -> Self {
        let buffer = Buffer::scratch(String::new(), BufferConfig::default())
            .expect("新建空白 Buffer 不应失败");
        Self::new(
            Rc::new(RefCell::new(buffer)),
            EditorMode::AutoHeight {
                min_lines,
                max_lines,
            },
            cx,
        )
    }

    pub(crate) fn for_buffer(buffer: Rc<RefCell<Buffer>>, cx: &mut Context<Self>) -> Self {
        Self::new(buffer, EditorMode::Full, cx)
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
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

    pub(super) fn set_caret(&mut self, offset: ByteOffset) {
        self.composition = None;
        self.selections = SelectionSet::caret(offset);
    }

    pub(super) fn set_input_layout(&mut self, layout: EditorInputLayout) {
        self.input_layout = Some(layout);
    }

    fn new(buffer: Rc<RefCell<Buffer>>, mode: EditorMode, cx: &mut Context<Self>) -> Self {
        let display_map = DisplayMap::new(buffer.borrow().snapshot());
        Self {
            buffer,
            display_map,
            mode,
            selections: SelectionSet::default(),
            selection_history: SelectionHistory::default(),
            scroll_manager: ScrollManager::default(),
            composition: None,
            input_layout: None,
            focus: cx.focus_handle(),
        }
    }

    fn selection_for_utf16_range(&self, range: Range<usize>) -> Option<SelectionSet> {
        let snapshot = self.buffer.borrow().snapshot();
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
            let Some(selection) = self.selection_for_utf16_range(range) else {
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
        let outcome = self.buffer.borrow_mut().insert_at_selections(
            &targets,
            &text,
            TransactionMetadata::new(TransactionSource::Programmatic).with_description("输入文本"),
        );
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
                    .set_snapshot(self.buffer.borrow().snapshot());
                self.input_layout = None;
                cx.notify();
            }
            Err(error) => eprintln!("Editor 输入事务失败：{error}"),
        }
    }
}

impl Render for Editor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.display_map
            .set_snapshot(self.buffer.borrow().snapshot());

        let line_height = typography::editor_line();
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
            .font(typography::editor_font())
            .text_size(typography::editor())
            .line_height(line_height)
            .text_color(color::current().gray.s[8])
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
            let Some(selection) = self.selection_for_utf16_range(range) else {
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
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.input_layout
            .as_ref()?
            .bounds_for_utf16_range(range_utf16)
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
    use gpui::{AppContext, Bounds, TestAppContext, point, px};
    use zcv_engine::{BufferConfig, ByteOffset, DisplayColumn, SelectionSet, TransactionId};

    use super::*;
    use crate::editor::display_map::{DisplayPoint, DisplayRow};

    fn buffer_text(buffer: &Rc<RefCell<Buffer>>) -> String {
        let buffer = buffer.borrow();
        buffer
            .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
            .expect("完整测试 Buffer 应可读取")
            .as_str()
            .to_owned()
    }

    #[gpui::test]
    fn editors_share_buffer_but_keep_view_state_independent(cx: &mut TestAppContext) {
        let buffer = Rc::new(RefCell::new(
            Buffer::scratch("abc".to_string(), BufferConfig::default())
                .expect("测试 Buffer 应能创建"),
        ));
        let first = cx.new(|cx| Editor::for_buffer(Rc::clone(&buffer), cx));
        let second = cx.new(|cx| Editor::for_buffer(Rc::clone(&buffer), cx));

        cx.update_entity(&first, |editor, _| {
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
            editor
                .buffer
                .borrow_mut()
                .insert(ByteOffset::new(3), "d")
                .expect("共享 Buffer 编辑应成功");
        });

        cx.read_entity(&second, |editor, _| {
            assert_eq!(editor.mode, EditorMode::Full);
            assert!(Rc::ptr_eq(&editor.buffer, &buffer));
            assert_eq!(editor.buffer.borrow().len_bytes(), ByteOffset::new(4));
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

        let single_buffer = cx.read_entity(&single_line, |editor, _| {
            assert_eq!(editor.mode, EditorMode::SingleLine);
            assert_eq!(editor.selections, SelectionSet::default());
            assert_eq!(
                editor.display_map.version(),
                editor.buffer.borrow().version()
            );
            let _focus = editor.focus_handle();
            Rc::clone(&editor.buffer)
        });
        let auto_height_buffer = cx.read_entity(&auto_height, |editor, _| {
            assert_eq!(
                editor.mode,
                EditorMode::AutoHeight {
                    min_lines: 2,
                    max_lines: Some(6),
                }
            );
            Rc::clone(&editor.buffer)
        });

        assert!(!Rc::ptr_eq(&single_buffer, &auto_height_buffer));
    }

    #[gpui::test]
    fn editor_element_renders_multiline_unicode_text(cx: &mut TestAppContext) {
        let buffer = Rc::new(RefCell::new(
            Buffer::scratch("a你\n😀b".to_string(), BufferConfig::default())
                .expect("测试 Buffer 应能创建"),
        ));
        let (editor, cx) = cx.add_window_view({
            let buffer = Rc::clone(&buffer);
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
        let buffer = Rc::new(RefCell::new(
            Buffer::scratch(String::new(), BufferConfig::default()).expect("测试 Buffer 应能创建"),
        ));
        let (editor, cx) = cx.add_window_view({
            let buffer = Rc::clone(&buffer);
            move |_, cx| Editor::for_buffer(buffer, cx)
        });

        cx.simulate_click(point(px(4.), px(12.)), gpui::Modifiers::default());
        cx.simulate_input("中😀e\u{301}");

        assert_eq!(buffer_text(&buffer), "中😀e\u{301}");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(
                editor.selections.primary().head(),
                ByteOffset::new("中😀e\u{301}".len())
            );
            assert!(editor.composition.is_none());
        });
    }

    #[gpui::test]
    fn marked_text_stays_out_of_buffer_and_unmark_commits_it(cx: &mut TestAppContext) {
        let buffer = Rc::new(RefCell::new(
            Buffer::scratch("ab".to_owned(), BufferConfig::default())
                .expect("测试 Buffer 应能创建"),
        ));
        let (editor, cx) = cx.add_window_view({
            let buffer = Rc::clone(&buffer);
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

        assert_eq!(buffer_text(&buffer), "ab");
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

        assert_eq!(buffer_text(&buffer), "a中文😀b");
        cx.read_entity(&editor, |editor, _| {
            assert!(editor.composition.is_none());
            assert_eq!(editor.selections.primary().head(), ByteOffset::new(11));
        });
    }

    #[gpui::test]
    fn marked_text_can_cancel_and_committed_range_uses_utf16_offsets(cx: &mut TestAppContext) {
        let buffer = Rc::new(RefCell::new(
            Buffer::scratch("a😀b".to_owned(), BufferConfig::default())
                .expect("测试 Buffer 应能创建"),
        ));
        let (editor, cx) = cx.add_window_view({
            let buffer = Rc::clone(&buffer);
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

        assert_eq!(buffer_text(&buffer), "a你b");
        cx.read_entity(&editor, |editor, _| {
            assert_eq!(editor.selections.primary().head(), ByteOffset::new(4));
        });
    }
}
