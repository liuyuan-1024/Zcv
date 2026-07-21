//! EditableText —— 可嵌入文本编辑器 Entity 组件。
//!
//! 自包含、不依赖 HostEffect 中介。通过 zcv-engine Buffer 管理文本数据，
//! 跨帧持有 buffer、光标等状态。字符输入走 `on_key_down`，命令走 action 系统。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    ClipboardItem, Context, CursorStyle, ElementId, FocusHandle, Render, Window, actions, div,
    prelude::*, px,
};
use zcv_engine::{Buffer, BufferConfig, ByteOffset, TextRange};

use crate::theme::{color, typography};

// ═══ 1. Action 定义 ═══════════════════════════════════════

actions!(
    editor,
    [
        Undo,  // cmd-z
        Redo,  // cmd-shift-z
        Cut,   // cmd-x
        Copy,  // cmd-c
        Paste, // cmd-v
    ]
);

// ═══ 2. Struct + constructor ══════════════════════════════

/// 可嵌入文本编辑器。
pub(crate) struct EditableText {
    buffer: Rc<RefCell<Buffer>>,
    cursor: Rc<Cell<ByteOffset>>,
    focus: FocusHandle,
    id: ElementId,
    placeholder: RefCell<String>,
    on_change: RefCell<Option<Rc<dyn Fn(&str, &mut Window, &mut gpui::App)>>>,
}

impl EditableText {
    /// 创建一个新的可编辑文本实体。
    pub fn new<T>(id: impl Into<ElementId>, cx: &mut Context<T>) -> Self {
        let buffer = Buffer::scratch(String::new(), BufferConfig::default())
            .expect("新建空白 Buffer 不应失败");

        Self {
            buffer: Rc::new(RefCell::new(buffer)),
            cursor: Rc::new(Cell::new(ByteOffset::ZERO)),
            focus: cx.focus_handle(),
            id: id.into(),
            placeholder: RefCell::new(String::new()),
            on_change: RefCell::new(None),
        }
    }

    /// 设置占位文本。
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        *self.placeholder.borrow_mut() = text.into();
        self
    }

    /// 设置文本变更回调（构造时链式调用）。
    pub fn on_change(
        mut self,
        handler: impl Fn(&str, &mut Window, &mut gpui::App) + 'static,
    ) -> Self {
        *self.on_change.borrow_mut() = Some(Rc::new(handler));
        self
    }

    /// 设置初始文本。
    pub fn set_text(&self, text: &str) {
        let mut buf = self.buffer.borrow_mut();
        let len = buf.len_bytes();
        if len > ByteOffset::ZERO {
            let _ = buf.delete(TextRange::new(ByteOffset::ZERO, len).unwrap());
        }
        if !text.is_empty() {
            let _ = buf.insert(ByteOffset::ZERO, text);
        }
        self.cursor.set(ByteOffset::new(text.len()));
    }

    /// 焦点句柄，供 Surface focus_on_open 使用。
    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// 构造后设置变更回调。
    pub fn set_on_change(&self, handler: impl Fn(&str, &mut Window, &mut gpui::App) + 'static) {
        *self.on_change.borrow_mut() = Some(Rc::new(handler));
    }

    /// 当前文本内容。
    pub fn text(&self) -> String {
        let buf = self.buffer.borrow();
        let len = buf.len_bytes();
        if len == ByteOffset::ZERO {
            return String::new();
        }
        if let Ok(range) = TextRange::new(ByteOffset::ZERO, len) {
            let snapshot = buf.snapshot();
            snapshot
                .slice_text(range)
                .map(|s| s.as_str().to_string())
                .unwrap_or_default()
        } else {
            String::new()
        }
    }
}

// ═══ 3. Action handler ════════════════════════════════════

impl EditableText {
    fn handle_undo(&mut self, _: &Undo, window: &mut Window, _cx: &mut Context<Self>) {
        // TODO: zcv-engine undo
        window.refresh();
    }

    fn handle_redo(&mut self, _: &Redo, window: &mut Window, _cx: &mut Context<Self>) {
        // TODO: zcv-engine redo
        window.refresh();
    }

    fn handle_cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let _ = self.buffer.borrow_mut().delete(cursor_range(&self.cursor));
        self.refresh_with_change(window, cx);
    }

    fn handle_copy(&mut self, _: &Copy, _window: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.text()));
    }

    fn handle_paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let Some(text) = item.text() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        let pos = self.cursor.get();
        let _ = self.buffer.borrow_mut().insert(pos, &text);
        self.cursor.set(ByteOffset::new(pos.get() + text.len()));
        self.refresh_with_change(window, cx);
    }

    fn refresh_with_change(&self, window: &mut Window, _cx: &mut Context<Self>) {
        window.refresh();
    }
}

// ═══ 4. Render ════════════════════════════════════════════

impl Render for EditableText {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let buffer = Rc::clone(&self.buffer);
        let cursor = Rc::clone(&self.cursor);
        let on_change: Option<Rc<dyn Fn(&str, &mut Window, &mut gpui::App)>> =
            self.on_change.borrow().clone();

        let display_text = self.text();
        let show_placeholder = display_text.is_empty();

        div()
            .id(self.id.clone())
            .track_focus(&self.focus)
            .key_context("Editor")
            .tab_index(0)
            .focusable()
            .flex()
            .items_center()
            .overflow_hidden()
            .h(typography::ui())
            .text_color(color::current().gray.s[8])
            .text_size(typography::ui())
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::handle_undo))
            .on_action(cx.listener(Self::handle_redo))
            .on_action(cx.listener(Self::handle_cut))
            .on_action(cx.listener(Self::handle_copy))
            .on_action(cx.listener(Self::handle_paste))
            .when(show_placeholder, |d| {
                let p = self.placeholder.borrow().clone();
                d.text_color(color::current().gray.s[5]).child(p)
            })
            .when(!show_placeholder, |d| {
                d.child(display_text).child(
                    div()
                        .w(px(1.5))
                        .h(typography::ui())
                        .bg(color::current().gray.s[8])
                        .ml(px(1.0)),
                )
            })
            .on_key_down(move |event, window, cx| {
                let handled = handle_key(&event.keystroke, &buffer, &cursor);
                if handled {
                    if let Some(ref cb) = on_change {
                        let text = read_text(&buffer);
                        if let Some(text) = text {
                            cb(&text, window, cx);
                        }
                    }
                    window.refresh();
                    cx.stop_propagation();
                }
            })
    }
}

// ═══ 5. 私有辅助函数 ═════════════════════════════════════

/// 光标所在范围的 TextRange（当前是单个光标点）。
fn cursor_range(cursor: &Cell<ByteOffset>) -> TextRange {
    let pos = cursor.get();
    TextRange::new(pos, ByteOffset::new(pos.get() + 1))
        .unwrap_or_else(|_| TextRange::new(pos, pos).unwrap())
}

/// 处理字符级键盘事件。
fn handle_key(
    keystroke: &gpui::Keystroke,
    buffer: &RefCell<Buffer>,
    cursor: &Cell<ByteOffset>,
) -> bool {
    // 有 cmd 修饰键的不算字符输入（走 action）
    if keystroke.modifiers.platform || keystroke.modifiers.control || keystroke.modifiers.alt {
        return false;
    }
    match keystroke.key.as_str() {
        "backspace" => {
            let pos = cursor.get();
            if pos > ByteOffset::ZERO {
                let start = ByteOffset::new(pos.get().saturating_sub(1));
                if let Ok(range) = TextRange::new(start, pos) {
                    let _ = buffer.borrow_mut().delete(range);
                    cursor.set(start);
                }
            }
            true
        }
        "delete" => {
            let pos = cursor.get();
            let len = buffer.borrow().len_bytes();
            if pos < len {
                let end = ByteOffset::new(pos.get().saturating_add(1).min(len.get()));
                if let Ok(range) = TextRange::new(pos, end) {
                    let _ = buffer.borrow_mut().delete(range);
                }
            }
            true
        }
        "left" => {
            let pos = cursor.get();
            if pos > ByteOffset::ZERO {
                cursor.set(ByteOffset::new(pos.get().saturating_sub(1)));
            }
            false
        }
        "right" => {
            let pos = cursor.get();
            let len = buffer.borrow().len_bytes();
            if pos < len {
                cursor.set(ByteOffset::new(pos.get() + 1));
            }
            false
        }
        "home" => {
            cursor.set(ByteOffset::ZERO);
            false
        }
        "end" => {
            cursor.set(buffer.borrow().len_bytes());
            false
        }
        key if key.len() == 1 => {
            let pos = cursor.get();
            let _ = buffer.borrow_mut().insert(pos, key);
            cursor.set(ByteOffset::new(pos.get() + key.len()));
            true
        }
        _ => false,
    }
}

/// 读 buffer 全文。
fn read_text(buffer: &RefCell<Buffer>) -> Option<String> {
    let buf = buffer.borrow();
    let len = buf.len_bytes();
    if len == ByteOffset::ZERO {
        return Some(String::new());
    }
    if let Ok(range) = TextRange::new(ByteOffset::ZERO, len) {
        let snapshot = buf.snapshot();
        snapshot
            .slice_text(range)
            .map(|s| s.as_str().to_string())
            .ok()
    } else {
        None
    }
}
