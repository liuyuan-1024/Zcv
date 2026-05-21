//! Workbench 窗口 UI 状态控制器。
//!
//! `app` 只产生命令层的 HostEffect；dock 展开、折叠、active panel 等窗口
//! 显示状态在这里解释和更新。

use super::docks::resize::{DockResize, DockResizeBounds, DockResizeEvent};
use super::docks::{bottom, left, right};
use super::state::{
    BottomBarState, DockAreaId, DockState, EditorState, PanelStack, WorkbenchState,
};
use crate::shell::features::PanelId;
use crate::shell::features::file_tree::FileTreeState;

use gpui::{Pixels, px};

pub(crate) struct WorkbenchController {
    left_dock: DockState,
    right_dock: DockState,
    bottom_dock: DockState,
    bottom_bar: BottomBarState,
    dock_resize: DockResize,
}

impl WorkbenchController {
    pub(crate) fn new() -> Self {
        Self {
            left_dock: DockState {
                collapsed: true,
                size: px(240.0),
                stack: PanelStack::new(left::PANELS.to_vec(), None),
            },
            right_dock: DockState {
                collapsed: true,
                size: px(240.0),
                stack: PanelStack::new(right::PANELS.to_vec(), None),
            },
            bottom_dock: DockState {
                collapsed: true,
                size: px(200.0),
                stack: PanelStack::new(bottom::PANELS.to_vec(), None),
            },
            bottom_bar: BottomBarState::default(),
            dock_resize: DockResize::default(),
        }
    }

    pub(crate) fn state(
        &self,
        project_title: String,
        has_project: bool,
        editor: EditorState,
        file_tree: FileTreeState,
    ) -> WorkbenchState {
        WorkbenchState {
            project_title,
            has_project,
            left_dock: self.left_dock.clone(),
            right_dock: self.right_dock.clone(),
            bottom_dock: self.bottom_dock.clone(),
            bottom_bar: self.bottom_bar.clone(),
            editor,
            file_tree,
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

    /// 该 panel 是否当前可见且为其 dock 的 active 项。Shell 用它决定切焦点。
    pub(crate) fn is_panel_active(&self, panel: PanelId) -> bool {
        for dock in [&self.left_dock, &self.right_dock, &self.bottom_dock] {
            if dock.stack.contains(panel) {
                return dock.is_visible() && dock.active_panel() == Some(panel);
            }
        }
        false
    }

    pub(crate) fn handle_dock_resize(&mut self, event: DockResizeEvent, bounds: DockResizeBounds) {
        let start_size = match event {
            DockResizeEvent::Start { area, .. } => Some(self.dock_state(area).size),
            DockResizeEvent::Drag { .. } | DockResizeEvent::End => None,
        };
        if let Some(update) = self.dock_resize.handle(event, start_size, bounds) {
            self.set_dock_size(update.area, update.size);
        }
    }

    fn set_dock_size(&mut self, area: DockAreaId, size: Pixels) {
        match area {
            DockAreaId::Left => self.left_dock.size = size,
            DockAreaId::Right => self.right_dock.size = size,
            DockAreaId::Bottom => self.bottom_dock.size = size,
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

    fn dock_state(&self, area: DockAreaId) -> &DockState {
        match area {
            DockAreaId::Left => &self.left_dock,
            DockAreaId::Right => &self.right_dock,
            DockAreaId::Bottom => &self.bottom_dock,
        }
    }
}

impl Default for WorkbenchController {
    fn default() -> Self {
        Self::new()
    }
}
