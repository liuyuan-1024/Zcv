//! PlaceholderPanel —— 尚未实现的面板占位。
//!
//! 每个面板对应一个 GPUI Entity，渲染简单的占位文字。
//! 实际面板组件就位后逐一替换。

use gpui::{Context, Render, Window, div, prelude::*};

use crate::theme::color;

macro_rules! placeholder_panel {
    ($name:ident, $label:expr) => {
        pub(crate) struct $name;
        impl Render for $name {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl gpui::IntoElement {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(color::current().gray.s[5])
                    .child($label)
            }
        }
    };
}

placeholder_panel!(VersionControlPanel, "版本控制");
placeholder_panel!(OutlinePanel, "大纲");
placeholder_panel!(TerminalPanel, "终端");
placeholder_panel!(DebugPanel, "调试");
placeholder_panel!(KeyboardShortcutsPanel, "快捷键");
