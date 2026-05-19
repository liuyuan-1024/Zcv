//! PanelHost —— workbench 的 panel 框架入口。
//!
//! 这里只负责按 panel id 分派到具体 feature。具体 feature 的状态、绘制与交互
//! 仍留在各自目录里。

use gpui::{AnyElement, IntoElement};

use crate::shell::features::file_tree::FileTreePanel;
use crate::shell::features::{
    PanelId, debug, file_tree, keyboard_shortcuts, outline, project_search, terminal,
    version_control,
};

/// Dock 调用 `PanelHost` 时透传给具体 panel 的运行态视图。
///
/// 这个上下文属于 workbench 的 panel 框架：它不拥有业务状态，只把已装配好的
/// feature runtime view 送到对应 panel。
#[derive(Clone, Copy)]
pub(crate) struct PanelContext<'a> {
    pub(crate) has_project: bool,
    pub(crate) file_tree: FileTreePanel<'a>,
}

pub(crate) struct PanelHost;

impl PanelHost {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn render(&self, id: PanelId, ctx: PanelContext<'_>) -> AnyElement {
        match id {
            PanelId::FileTree => file_tree::render(ctx).into_any_element(),
            PanelId::VersionControl => version_control::render().into_any_element(),
            PanelId::Outline => outline::render().into_any_element(),
            PanelId::ProjectSearch => project_search::render().into_any_element(),
            PanelId::Terminal => terminal::render().into_any_element(),
            PanelId::Debug => debug::render().into_any_element(),
            PanelId::KeyboardShortcuts => keyboard_shortcuts::render().into_any_element(),
        }
    }
}

impl Default for PanelHost {
    fn default() -> Self {
        Self::new()
    }
}
