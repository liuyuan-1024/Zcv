//! BottomBar —— 窗口级底部外壳（Entity）。
//!
//! action handler 通过 GPUI 全局状态 `LayoutRef` 访问布局控制器。

use gpui::{Action, AnyElement, Context, Div, Window, actions, div, prelude::*};

use crate::keymap::KeyBindings;
use crate::shared::Glyph;
use crate::theme::color;
use crate::workbench::PanelId;
use crate::workbench::layout::LayoutRef;

use super::frame::{BarEdge, BarRegionAlign, align_bar_region, bar_divider, bar_frame};

actions!(
    bottom_bar,
    [
        ToggleProjectTree,
        ToggleVersionControl,
        ToggleOutline,
        ToggleLanguageServer,
        ToggleDiagnostics,
        ToggleProjectSearch,
        ToggleTerminal,
        ToggleDebug,
        ToggleKeyboardShortcuts,
    ]
);

// ── BottomBar Entity ───────────────────────────────────────────────

pub(crate) struct BottomBar;

impl BottomBar {
    pub(crate) fn new(_cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl gpui::Render for BottomBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let is_active = |panel: PanelId| -> bool {
            cx.try_global::<LayoutRef>()
                .and_then(|r| r.0.upgrade())
                .map(|ctrl| ctrl.borrow().is_panel_active(panel))
                .unwrap_or(false)
        };
        let kb = cx.try_global::<KeyBindings>();

        bar_frame(BarEdge::Bottom)
            .id("bottom-bar")
            .child(region(
                leading_slots(&is_active, kb),
                BarRegionAlign::Leading,
            ))
            .child(region(
                trailing_slots(&is_active, kb),
                BarRegionAlign::Trailing,
            ))
    }
}

fn region(items: Vec<AnyElement>, align: BarRegionAlign) -> Div {
    let inner = div().flex().items_center().gap_2().children(items);
    align_bar_region(inner, align)
}

/// dispatch_action 封装：GPUI 要求 action 装箱。
macro_rules! dispatch {
    ($window:expr, $action:expr, $cx:expr) => {
        $window.dispatch_action(Box::new($action), $cx)
    };
}

fn leading_slots(is_active: &dyn Fn(PanelId) -> bool, kb: Option<&KeyBindings>) -> Vec<AnyElement> {
    let s = |name: &str| kb.and_then(|kb| kb.display_shortcut(name));

    join_groups(vec![
        vec![
            Glyph::icon("bottom-bar.project-tree", "icons/panels/project_tree.svg")
                .label("项目树")
                .shortcut(s(ToggleProjectTree.name()))
                .color(if is_active(PanelId::ProjectTree) {
                    color::glyph_active()
                } else {
                    color::glyph_default()
                })
                .on_click(|window, cx| dispatch!(window, ToggleProjectTree, cx))
                .into_any_element(),
            Glyph::icon(
                "bottom-bar.version-control",
                "icons/panels/version_control.svg",
            )
            .label("版本控制")
            .shortcut(s(ToggleVersionControl.name()))
            .color(if is_active(PanelId::VersionControl) {
                color::glyph_active()
            } else {
                color::glyph_default()
            })
            .on_click(|window, cx| dispatch!(window, ToggleVersionControl, cx))
            .into_any_element(),
            Glyph::icon("bottom-bar.outline", "icons/panels/outline.svg")
                .label("大纲")
                .shortcut(s(ToggleOutline.name()))
                .color(if is_active(PanelId::Outline) {
                    color::glyph_active()
                } else {
                    color::glyph_default()
                })
                .on_click(|window, cx| dispatch!(window, ToggleOutline, cx))
                .into_any_element(),
        ],
        vec![
            Glyph::icon(
                "bottom-bar.language-server",
                "icons/status/language_server.svg",
            )
            .label("语言服务器")
            .shortcut(s(ToggleLanguageServer.name()))
            .on_click(|window, cx| dispatch!(window, ToggleLanguageServer, cx))
            .into_any_element(),
            Glyph::icon_text(
                "bottom-bar.diagnostics",
                "icons/status/diagnostics.svg",
                "0",
            )
            .label("诊断")
            .shortcut(s(ToggleDiagnostics.name()))
            .on_click(|window, cx| dispatch!(window, ToggleDiagnostics, cx))
            .into_any_element(),
            Glyph::icon("bottom-bar.project-search", "icons/panels/search.svg")
                .label("项目搜索")
                .shortcut(s(ToggleProjectSearch.name()))
                .on_click(|window, cx| dispatch!(window, ToggleProjectSearch, cx))
                .into_any_element(),
        ],
    ])
}

fn trailing_slots(
    is_active: &dyn Fn(PanelId) -> bool,
    kb: Option<&KeyBindings>,
) -> Vec<AnyElement> {
    let s = |name: &str| kb.and_then(|kb| kb.display_shortcut(name));

    join_groups(vec![
        vec![
            Glyph::text("bottom-bar.cursor", "1:1")
                .label("光标位置")
                .into_any_element(),
            Glyph::text("bottom-bar.language", "Rust")
                .label("语言")
                .into_any_element(),
        ],
        vec![
            Glyph::icon("bottom-bar.terminal", "icons/panels/terminal.svg")
                .label("终端")
                .shortcut(s(ToggleTerminal.name()))
                .color(if is_active(PanelId::Terminal) {
                    color::glyph_active()
                } else {
                    color::glyph_default()
                })
                .on_click(|window, cx| dispatch!(window, ToggleTerminal, cx))
                .into_any_element(),
            Glyph::icon("bottom-bar.debug", "icons/panels/debug.svg")
                .label("调试")
                .shortcut(s(ToggleDebug.name()))
                .color(if is_active(PanelId::Debug) {
                    color::glyph_active()
                } else {
                    color::glyph_default()
                })
                .on_click(|window, cx| dispatch!(window, ToggleDebug, cx))
                .into_any_element(),
        ],
        vec![
            Glyph::icon(
                "bottom-bar.keyboard-shortcuts",
                "icons/panels/keyboard_shortcuts.svg",
            )
            .label("快捷键")
            .shortcut(s(ToggleKeyboardShortcuts.name()))
            .color(if is_active(PanelId::KeyboardShortcuts) {
                color::glyph_active()
            } else {
                color::glyph_default()
            })
            .on_click(|window, cx| dispatch!(window, ToggleKeyboardShortcuts, cx))
            .into_any_element(),
        ],
    ])
}

fn join_groups(groups: Vec<Vec<AnyElement>>) -> Vec<AnyElement> {
    let mut out = Vec::new();
    for group in groups {
        if !out.is_empty() {
            out.push(bar_divider().into_any_element());
        }
        out.extend(group);
    }
    out
}
