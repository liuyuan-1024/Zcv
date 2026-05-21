//! PanelHost —— workbench 的 panel 框架入口。
//!
//! 这里只负责按 panel id 分派到具体 feature。具体 feature 的状态、绘制与交互
//! 仍留在各自目录里。

use gpui::{AnyElement, IntoElement};

use crate::shell::KeyRequest;
use crate::shell::features::file_tree::FileTreePanel;
use crate::shell::features::{PanelId, PanelRuntimes, file_tree};

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
                .render(id, ctx.panel_key_request)
                .unwrap_or_else(|| gpui::div().into_any_element()),
        }
    }
}

impl Default for PanelHost {
    fn default() -> Self {
        Self::new()
    }
}
