//! 编辑器状态项：把 Editor 状态投影到底栏。
//!
//! 宿主只调用 [`install_status_items`] 完成注册，不直接依赖具体状态项类型；
//! 状态项在 `set_active_pane_item` 中订阅 Editor 并在没有活动编辑器时隐藏。

use gpui::{Context, Entity, Render, Subscription, Window, div, prelude::*};
use zcv_ui::Button;
use zcv_workspace::{ItemHandle, StatusItemView, Workspace};

use crate::Editor;

/// 底栏光标位置显示。
struct CursorPosition {
    cursor_text: String,
    _subscription: Option<Subscription>,
}

impl CursorPosition {
    fn new() -> Self {
        Self {
            cursor_text: String::new(),
            _subscription: None,
        }
    }
}

impl StatusItemView for CursorPosition {
    fn set_active_pane_item(&mut self, item: Option<&dyn ItemHandle>, cx: &mut Context<Self>) {
        self._subscription = None;

        if let Some(editor) = item.and_then(|item| item.act_as::<Editor>(cx)) {
            self._subscription = Some(cx.observe(&editor, |this, editor, cx| {
                this.cursor_text = editor.read(cx).cursor_text(cx);
                cx.notify();
            }));
            self.cursor_text = editor.read(cx).cursor_text(cx);
        } else {
            self.cursor_text = String::new();
        }

        cx.notify();
    }
}

impl Render for CursorPosition {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl gpui::IntoElement {
        // 无活动编辑器时隐藏（终端、空工作区等场景不显示空按钮）。
        if self.cursor_text.is_empty() {
            return div().into_any_element();
        }
        Button::text("status-bar.cursor", self.cursor_text.clone())
            .label("跳转到行/列")
            .into_any_element()
    }
}

/// 底栏当前语言显示。
struct ActiveBufferLanguage {
    language: String,
    _subscription: Option<Subscription>,
}

impl ActiveBufferLanguage {
    fn new() -> Self {
        Self {
            language: String::new(),
            _subscription: None,
        }
    }

    fn sync_language(&mut self, editor: &Entity<Editor>, cx: &mut Context<Self>) {
        self.language = editor
            .read(cx)
            .language_name(cx)
            .map(|name| name.to_owned())
            .unwrap_or_default();
    }
}

impl StatusItemView for ActiveBufferLanguage {
    fn set_active_pane_item(&mut self, item: Option<&dyn ItemHandle>, cx: &mut Context<Self>) {
        self._subscription = None;

        if let Some(editor) = item.and_then(|item| item.act_as::<Editor>(cx)) {
            self._subscription = Some(cx.observe(&editor, |this, editor, cx| {
                this.sync_language(&editor, cx);
                cx.notify();
            }));
            self.sync_language(&editor, cx);
        } else {
            self.language = String::new();
        }

        cx.notify();
    }
}

impl Render for ActiveBufferLanguage {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl gpui::IntoElement {
        // 无活动编辑器或无语言时隐藏（终端、空工作区等场景不显示空按钮）。
        if self.language.is_empty() {
            return div().into_any_element();
        }
        Button::text("status-bar.language", self.language.clone())
            .label("当前语言")
            .into_any_element()
    }
}

/// 把编辑器状态项注册到工作区底栏右侧。
pub fn install_status_items(workspace: &mut Workspace, cx: &mut Context<Workspace>) {
    let status_bar = workspace.status_bar().clone();
    status_bar.update(cx, |bar, cx| {
        bar.add_right_item(cx.new(|_| CursorPosition::new()), cx);
        bar.add_right_item(cx.new(|_| ActiveBufferLanguage::new()), cx);
    });
}

#[cfg(test)]
#[path = "test/status_items_tests.rs"]
mod tests;
