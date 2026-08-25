//! CursorPosition —— 底栏光标位置显示。
//!
//! 实现 StatusItemView，在 set_active_pane_item 中订阅 Editor 变化，读取 cursor_text 并更新显示。

use gpui::{Context, Render, Subscription, Window, prelude::*};
use zcv_editor::Editor;
use zcv_ui::Button;
use zcv_workspace::ItemHandle;
use zcv_workspace::StatusItemView;

pub(crate) struct CursorPosition {
    cursor_text: String,
    _subscription: Option<Subscription>,
}

impl CursorPosition {
    pub(crate) fn new() -> Self {
        Self {
            cursor_text: String::new(),
            _subscription: None,
        }
    }
}

impl StatusItemView for CursorPosition {
    fn set_active_pane_item(&mut self, item: Option<&dyn ItemHandle>, cx: &mut Context<Self>) {
        // 取消旧订阅
        self._subscription = None;

        if let Some(editor) = item.and_then(|item| item.act_as::<Editor>(cx)) {
            // 订阅 Editor 变化（选区移动、编辑等都会触发 notify）
            self._subscription = Some(cx.observe(&editor, |this, ed, cx| {
                this.cursor_text = ed.read(cx).cursor_text();
                cx.notify();
            }));
            // 立即读取当前值
            self.cursor_text = editor.read(cx).cursor_text();
        } else {
            self.cursor_text = String::new();
        }

        cx.notify();
    }
}

impl Render for CursorPosition {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl gpui::IntoElement {
        Button::text("status-bar.cursor", self.cursor_text.clone())
            .label("跳转到行/列")
            .disabled(true)
            .into_any_element()
    }
}
