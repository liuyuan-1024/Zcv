//! ActiveBufferLanguage —— 底栏语言显示。
//!
//! 实现 StatusItemView，在 set_active_pane_item 中订阅 Editor 变化，显示当前编辑器语言名（当前仅取 Editor 的语言名；文件扩展名/首行检测为规划能力，见 Zed 的 active_buffer_language）。

use gpui::{Context, Entity, Render, Subscription, Window, prelude::*};
use zcv_editor::Editor;
use zcv_ui::Button;
use zcv_workspace::ItemHandle;
use zcv_workspace::StatusItemView;

pub(crate) struct ActiveBufferLanguage {
    language: String,
    _subscription: Option<Subscription>,
}

impl ActiveBufferLanguage {
    pub(crate) fn new() -> Self {
        Self {
            language: String::new(),
            _subscription: None,
        }
    }
}

impl StatusItemView for ActiveBufferLanguage {
    fn set_active_pane_item(&mut self, item: Option<&dyn ItemHandle>, cx: &mut Context<Self>) {
        // 取消旧订阅
        self._subscription = None;

        if let Some(editor) = item.and_then(|item| item.act_as::<Editor>(cx)) {
            // 订阅 Editor 变化（编辑可能改变首行 shebang，从而影响语言检测）
            self._subscription = Some(cx.observe(&editor, |this, ed, cx| {
                this.sync_language(&ed, cx);
            }));
            // 立即检测
            self.sync_language(&editor, cx);
        } else {
            self.language = String::new();
        }

        cx.notify();
    }
}

impl ActiveBufferLanguage {
    fn sync_language(&mut self, editor: &Entity<Editor>, cx: &mut Context<Self>) {
        let editor_ref = editor.read(cx);

        self.language = editor_ref
            .language_name(cx)
            .map(|name| name.to_owned())
            .unwrap_or_default();
    }
}

impl Render for ActiveBufferLanguage {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl gpui::IntoElement {
        Button::text("status-bar.language", self.language.clone())
            .label("当前语言")
            .into_any_element()
    }
}
