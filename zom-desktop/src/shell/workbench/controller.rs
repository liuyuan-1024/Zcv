//! Workbench 窗口 UI 状态控制器。
//!
//! `app` 只产生命令层的 HostEffect；dock 展开、折叠、active panel 等窗口
//! 显示状态在这里解释和更新。

use crate::shell::features::PanelId;

use super::regions::{bottom_dock, left_dock, right_dock};
use super::state::{BottomBarState, DockState, EditorState, PanelStack, WorkbenchState};

use gpui::px;

pub(crate) struct WorkbenchController {
    left_dock: DockState,
    right_dock: DockState,
    bottom_dock: DockState,
    bottom_bar: BottomBarState,
}

impl WorkbenchController {
    pub(crate) fn new() -> Self {
        let mut controller = Self {
            left_dock: DockState {
                collapsed: true,
                size: px(240.0),
                stack: PanelStack::new(left_dock::PANELS.to_vec(), None),
            },
            right_dock: DockState {
                collapsed: true,
                size: px(240.0),
                stack: PanelStack::new(right_dock::PANELS.to_vec(), None),
            },
            bottom_dock: DockState {
                collapsed: true,
                size: px(200.0),
                stack: PanelStack::new(bottom_dock::PANELS.to_vec(), None),
            },
            bottom_bar: BottomBarState::default(),
        };
        controller.toggle_panel(PanelId::FileTree);
        controller
    }

    pub(crate) fn state(&self, project_title: String, editor: EditorState) -> WorkbenchState {
        WorkbenchState {
            project_title,
            left_dock: self.left_dock.clone(),
            right_dock: self.right_dock.clone(),
            bottom_dock: self.bottom_dock.clone(),
            bottom_bar: self.bottom_bar.clone(),
            editor,
        }
    }

    pub(crate) fn toggle_panel(&mut self, panel: PanelId) {
        let Some(dock) = self.dock_hosting_mut(panel) else {
            return;
        };
        if dock.stack.active() == Some(panel) && !dock.collapsed {
            dock.collapsed = true;
        } else {
            dock.collapsed = false;
            dock.stack.active = Some(panel);
        }
    }

    fn dock_hosting_mut(&mut self, panel: PanelId) -> Option<&mut DockState> {
        for dock in [
            &mut self.left_dock,
            &mut self.right_dock,
            &mut self.bottom_dock,
        ] {
            if dock.stack.contains(panel) {
                return Some(dock);
            }
        }
        None
    }
}

impl Default for WorkbenchController {
    fn default() -> Self {
        Self::new()
    }
}
