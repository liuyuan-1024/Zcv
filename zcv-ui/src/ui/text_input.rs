//! TextInput —— 轻量单行文本输入组件（Picker 搜索框等场景）。
//!
//! 对齐 Zed 的 `ui_input::TextInput`：自足实现 gpui 的文本输入协议（[`EntityInputHandler`]，字符与 IME 组合文本经平台路由到本组件），不依赖编辑器 crate。
//! key context 复用 "Editor"：方向键/导航键（down/up/enter）经 keymap 冒泡给宿主（如 Picker 做列表导航），左右/退格/删除由本组件自行处理。

use std::ops::Range;

use gpui::{
    App, Bounds, Context, EventEmitter, FocusHandle, Pixels, Point, Render, TextRun,
    UTF16Selection, Window, div, point, prelude::*, px, size,
};
use zcv_actions::{Backspace, Delete, MoveLeft, MoveRight};
use zcv_theme::{color, typography};

/// 文本内容变化事件。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TextInputEvent {
    TextChanged,
}

pub struct TextInput {
    focus: FocusHandle,
    text: String,
    placeholder: String,
    /// 光标位置（UTF-16 码元偏移，平台输入协议使用）。
    cursor: usize,
    /// 输入法组合文本范围（UTF-16）。
    marked_range: Option<Range<usize>>,
    /// 渲染期记录的光标像素偏移（IME 候选窗定位用）。
    cursor_pixel_offset: Option<Pixels>,
    line_height: Pixels,
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl TextInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            text: String::new(),
            placeholder: String::new(),
            cursor: 0,
            marked_range: None,
            cursor_pixel_offset: None,
            line_height: typography::ui_line(),
        }
    }

    pub fn set_placeholder_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.placeholder = text.into();
        cx.notify();
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.text = text.into();
        self.cursor = self.text.encode_utf16().count();
        self.marked_range = None;
        cx.notify();
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_text("", cx);
    }

    // ══ 坐标转换 ══

    /// UTF-16 偏移 → UTF-8 字节偏移；落在代理对中间时对齐到字符末尾。
    fn utf16_to_utf8_offset(&self, utf16_offset: usize) -> Option<usize> {
        let mut utf16 = 0;
        for (byte_offset, ch) in self.text.char_indices() {
            if utf16 == utf16_offset {
                return Some(byte_offset);
            }
            utf16 += ch.len_utf16();
            if utf16 > utf16_offset {
                return Some(byte_offset + ch.len_utf8());
            }
        }
        (utf16 == utf16_offset).then_some(self.text.len())
    }

    /// UTF-8 字节偏移 → UTF-16 偏移。
    fn utf8_to_utf16_offset(&self, byte_offset: usize) -> usize {
        self.text[..byte_offset].encode_utf16().count()
    }

    /// UTF-16 范围 → UTF-8 字节范围。
    fn utf16_range_to_utf8(&self, range: Range<usize>) -> Option<Range<usize>> {
        let start = self.utf16_to_utf8_offset(range.start)?;
        let end = self.utf16_to_utf8_offset(range.end)?;
        Some(start..end)
    }

    // ══ 文本编辑 ══

    /// 当前编辑目标范围：组合文本存在时覆盖 marked 范围，否则为光标点。
    fn edit_range(&self) -> Range<usize> {
        self.marked_range
            .clone()
            .unwrap_or(self.cursor..self.cursor)
    }

    /// 用替换文本覆盖指定 UTF-16 范围，光标移到替换后位置。
    fn replace_range(&mut self, range_utf16: Range<usize>, replacement: &str) {
        let Some(range) = self.utf16_range_to_utf8(range_utf16) else {
            return;
        };
        self.text.replace_range(range.clone(), replacement);
        self.cursor = self.utf8_to_utf16_offset(range.start) + replacement.encode_utf16().count();
        self.marked_range = None;
    }

    fn delete_backward(&mut self) {
        let Some(cursor) = self.utf16_to_utf8_offset(self.cursor) else {
            return;
        };
        let Some((start, ch)) = self.text[..cursor].char_indices().next_back() else {
            return;
        };
        self.text.replace_range(start..cursor, "");
        self.cursor -= ch.len_utf16();
        self.marked_range = None;
    }

    fn delete_forward(&mut self) {
        let Some(cursor) = self.utf16_to_utf8_offset(self.cursor) else {
            return;
        };
        let Some(ch) = self.text[cursor..].chars().next() else {
            return;
        };
        self.text.replace_range(cursor..cursor + ch.len_utf8(), "");
        self.marked_range = None;
    }

    fn move_left(&mut self) {
        let Some(cursor) = self.utf16_to_utf8_offset(self.cursor) else {
            return;
        };
        if let Some((_, ch)) = self.text[..cursor].char_indices().next_back() {
            self.cursor -= ch.len_utf16();
        }
        self.marked_range = None;
    }

    fn move_right(&mut self) {
        let Some(cursor) = self.utf16_to_utf8_offset(self.cursor) else {
            return;
        };
        if let Some(ch) = self.text[cursor..].chars().next() {
            self.cursor += ch.len_utf16();
        }
        self.marked_range = None;
    }

    // ══ Action handler ══

    fn handle_backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_backward();
        cx.emit(TextInputEvent::TextChanged);
        cx.notify();
    }

    fn handle_delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_forward();
        cx.emit(TextInputEvent::TextChanged);
        cx.notify();
    }

    fn handle_move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_left();
        cx.notify();
    }

    fn handle_move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_right();
        cx.notify();
    }
}

impl gpui::Focusable for TextInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl gpui::EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let utf8 = self.utf16_range_to_utf8(range.clone())?;
        adjusted_range.replace(range);
        Some(self.text[utf8].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.cursor..self.cursor,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range.clone()
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 组合提交（IME 确认）时 range 为 marked 范围；直接输入时覆盖光标处。
        self.replace_range(range_utf16.unwrap_or_else(|| self.edit_range()), text);
        cx.emit(TextInputEvent::TextChanged);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16.unwrap_or_else(|| self.edit_range());
        let Some(utf8) = self.utf16_range_to_utf8(range) else {
            return;
        };
        self.text.replace_range(utf8.clone(), new_text);
        // 组合文本保持 marked 状态，随候选词更新而替换。
        let start = self.utf8_to_utf16_offset(utf8.start);
        self.marked_range = Some(start..start + new_text.encode_utf16().count());
        self.cursor = new_selected_range
            .map(|selection| start + selection.end)
            .unwrap_or(start + new_text.encode_utf16().count());
        cx.emit(TextInputEvent::TextChanged);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // IME 候选窗跟随光标：x 用渲染期测量的光标像素偏移。
        let x = self.cursor_pixel_offset?;
        Some(Bounds::new(
            point(element_bounds.origin.x + x, element_bounds.origin.y),
            size(px(2.0), self.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        // 暂不支持点击定位光标（搜索框场景无需）。
        None
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 测量光标前的文本宽度，供 IME 候选窗定位。
        let font = typography::ui_font();
        let text_size = typography::ui();
        let text_color: gpui::Hsla = color::current(cx).text.into();
        let cursor_byte = self
            .utf16_to_utf8_offset(self.cursor)
            .unwrap_or(self.text.len());
        let prefix = &self.text[..cursor_byte];
        self.cursor_pixel_offset = if prefix.is_empty() {
            Some(Pixels::ZERO)
        } else {
            let run = TextRun {
                len: prefix.len(),
                font: font.clone(),
                color: text_color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped =
                window
                    .text_system()
                    .shape_line(prefix.to_owned().into(), text_size, &[run], None);
            Some(shaped.width)
        };

        let suffix = &self.text[cursor_byte..];
        let placeholder_visible = self.text.is_empty();
        let cursor_visible = self.focus.is_focused(window);

        let base_style = div()
            .flex()
            .items_center()
            .text_size(text_size)
            .line_height(self.line_height);

        div()
            .id("text-input")
            .track_focus(&self.focus)
            .key_context("Editor")
            .on_action(cx.listener(Self::handle_backspace))
            .on_action(cx.listener(Self::handle_delete))
            .on_action(cx.listener(Self::handle_move_left))
            .on_action(cx.listener(Self::handle_move_right))
            .child(
                base_style
                    .when(placeholder_visible, |el| {
                        el.text_color(color::current(cx).text_placeholder)
                            .child(self.placeholder.clone())
                    })
                    .when(!placeholder_visible, |el| {
                        el.text_color(color::current(cx).text)
                            .child(prefix.to_owned())
                            .when(cursor_visible, |el| {
                                el.child(div().w(px(1.0)).h(self.line_height).bg(text_color))
                            })
                            .child(suffix.to_owned())
                    }),
            )
    }
}
