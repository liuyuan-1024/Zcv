//! PanelHost —— workbench 的 panel 框架入口。
//!
//! 这里负责按 panel id 分派到具体 feature，并提供 panel 通用的承载小件
//! （焦点宿主、骨架占位）。具体 feature 的状态、绘制与交互仍留在各自目录里。

use std::rc::Rc;

use gpui::{AnyElement, Div, FocusHandle, SharedString, div, prelude::*};

use crate::shell::features::file_tree::FileTreePanel;
use crate::shell::features::{PanelId, PanelRuntimes, file_tree};
use crate::shell::shared::theme::{color, typography};
use crate::shell::{CommandTitleLookup, KeyRequest, normalized_chord};

/// Dock 调用 `PanelHost` 时透传给具体 panel 的运行态视图。
///
/// 这个上下文属于 workbench 的 panel 框架：它不拥有业务状态，只把已装配好的
/// feature runtime view 送到对应 panel。
#[derive(Clone, Copy)]
pub(crate) struct PanelContext<'a> {
    pub(crate) has_project: bool,
    pub(crate) file_tree: FileTreePanel<'a>,
    pub(crate) panel_runtimes: &'a PanelRuntimes,
    pub(crate) panel_key_request: &'a KeyRequest,
    pub(crate) command_title_lookup: &'a CommandTitleLookup,
}

pub(crate) struct PanelHost;

impl PanelHost {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn render(&self, id: PanelId, ctx: PanelContext<'_>) -> AnyElement {
        match id {
            PanelId::FileTree => file_tree::render(ctx).into_any_element(),
            _ => ctx
                .panel_runtimes
                .render(id, ctx.panel_key_request, ctx.command_title_lookup)
                .unwrap_or_else(|| gpui::div().into_any_element()),
        }
    }
}

impl Default for PanelHost {
    fn default() -> Self {
        Self::new()
    }
}

/// 把 panel 正文包进统一的焦点宿主：track focus + tab_index + 键路由。
/// 属于「承载」职责，故由 workbench 提供，feature 只传入自己的正文。
pub(crate) fn render_focus_host(
    focus: &FocusHandle,
    key_request: &KeyRequest,
    body: AnyElement,
) -> Div {
    let key_request = Rc::clone(key_request);

    div()
        .size_full()
        .track_focus(focus)
        .tab_index(0)
        .on_key_down(move |event, window, cx| {
            if key_request(normalized_chord(&event.keystroke), window, cx) {
                cx.stop_propagation();
            }
        })
        .child(body)
}

/// 第一版骨架阶段的 panel 占位体。真实 panel 接入自己的 UI 后不再使用它。
pub(crate) fn placeholder(hint: impl Into<SharedString>) -> Div {
    div().flex().flex_col().size_full().child(
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_size(typography::ui())
            .text_color(color::gray::g60())
            .child(hint.into()),
    )
}
