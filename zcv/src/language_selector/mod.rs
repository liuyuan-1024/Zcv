//! ActiveBufferLanguage —— 底栏语言显示。
//!
//! 实现 StatusItemView，在 set_active_editor 中订阅 Editor 变化，根据文件路径和 Buffer 首行检测语言并更新显示。

use gpui::{Context, Entity, Render, Subscription, Window, prelude::*};

use crate::ui::Glyph;
use crate::workspace::StatusItemView;
use zcv_editor::Editor;

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
    fn set_active_editor(&mut self, editor: Option<&Entity<Editor>>, cx: &mut Context<Self>) {
        // 取消旧订阅
        self._subscription = None;

        if let Some(editor) = editor {
            // 订阅 Editor 变化（编辑可能改变首行 shebang，从而影响语言检测）
            self._subscription = Some(cx.observe(editor, |this, ed, cx| {
                this.sync_language(&ed, cx);
            }));
            // 立即检测
            self.sync_language(editor, cx);
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
        Glyph::text("status-bar.language", self.language.clone())
            .label("选择语言")
            .into_any_element()
    }
}
