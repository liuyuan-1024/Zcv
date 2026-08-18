//! 占位面板：大纲/终端/调试/快捷键（后续接入真实功能）。

use gpui::{App, Context, FocusHandle, Render, Window, div, prelude::*};
use zcv_theme::color;
use zcv_workspace::Panel;

macro_rules! make_placeholder_panel {
    ($name:ident, $persistent:expr, $icon:expr, $label:expr) => {
        pub(crate) struct $name {
            focus: FocusHandle,
        }

        impl $name {
            pub(crate) fn new(cx: &mut Context<Self>) -> Self {
                Self {
                    focus: cx.focus_handle(),
                }
            }
        }

        impl Panel for $name {
            fn icon() -> &'static str {
                $icon
            }
            fn label() -> &'static str {
                $label
            }
            fn persistent_name() -> &'static str {
                $persistent
            }
            fn focus_handle(&self, _cx: &App) -> FocusHandle {
                self.focus.clone()
            }
        }

        impl Render for $name {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .track_focus(&self.focus)
                    .key_context($persistent)
                    .tab_index(0)
                    .text_color(color::current(cx).text_placeholder)
                    .child($label)
            }
        }
    };
}

make_placeholder_panel!(OutlinePanel, "outline", "icons/list_tree.svg", "大纲");

make_placeholder_panel!(TerminalPanel, "terminal", "icons/terminal.svg", "终端");

make_placeholder_panel!(DebugPanel, "debug", "icons/debug.svg", "调试");

make_placeholder_panel!(
    KeyboardShortcutsPanel,
    "keyboard-shortcuts",
    "icons/keyboard.svg",
    "快捷键"
);
