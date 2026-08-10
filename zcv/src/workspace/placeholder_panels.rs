//! 占位面板：大纲/终端/调试/快捷键（后续接入真实功能）。
//!
//! 面板 toggle action 由 zcv-workspace 的 dock 模块声明，这里只绑定具体类型。

use gpui::{App, Context, FocusHandle, Render, Window, div, prelude::*};
use zcv_theme::color;
use zcv_workspace::{Panel, ToggleDebug, ToggleKeyboardShortcuts, ToggleOutline, ToggleTerminal};

macro_rules! make_placeholder_panel {
    ($name:ident, $toggle_action:ty, $persistent:expr, $icon:expr, $label:expr) => {
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
            type ToggleAction = $toggle_action;

            fn icon() -> &'static str {
                $icon
            }
            fn label() -> &'static str {
                $label
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

make_placeholder_panel!(
    OutlinePanel,
    ToggleOutline,
    "Outline",
    "icons/list_tree.svg",
    "大纲"
);

make_placeholder_panel!(
    TerminalPanel,
    ToggleTerminal,
    "Terminal",
    "icons/terminal.svg",
    "终端"
);

make_placeholder_panel!(DebugPanel, ToggleDebug, "Debug", "icons/debug.svg", "调试");

make_placeholder_panel!(
    KeyboardShortcutsPanel,
    ToggleKeyboardShortcuts,
    "KeyboardShortcuts",
    "icons/keyboard.svg",
    "快捷键"
);
