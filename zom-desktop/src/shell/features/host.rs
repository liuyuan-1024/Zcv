//! `PanelHost` —— 每窗口持有所有 panel 实例的索引（手册 20.3）。
//!
//! 第一版骨架：panel 渲染本身是无状态函数；`PanelHost` 退化为「按 id
//! 分派到对应 panel 的 render」的薄薄一层。一旦 panel 升级为 GPUI
//! Entity，本结构升级为 `Entity<FileTreePanel>` 等字段集合，但对外 API
//! `render(id)` 不变。

use gpui::{AnyElement, IntoElement};

use super::{
    debug, file_tree, keyboard_shortcuts, outline, project_search, terminal, version_control,
};
use crate::shell::features::PanelId;

pub(crate) struct PanelHost;

impl PanelHost {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn render(&self, id: PanelId) -> AnyElement {
        match id {
            PanelId::FileTree => file_tree::render().into_any_element(),
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
