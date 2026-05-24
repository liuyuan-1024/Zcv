//! Workbench 窗口 UI 状态控制器。
//!
//! `app` 只产生命令层的 HostEffect；dock 展开、折叠、active panel 等窗口
//! 显示状态在这里解释和更新。

use super::docks::resize::{DockResize, DockResizeBounds, DockResizeEvent};
use super::docks::{bottom, left, right};
use super::state::{
    BottomBarState, DockAreaId, DockState, EditorState, PanelStack, WorkbenchState,
};
use crate::shell::features::panels::PanelId;
use crate::shell::features::panels::file_tree::FileTreeState;
use crate::shell::features::panels::search::SearchState;

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
        search: SearchState,
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
            search,
        }
    }

    /// 显示并激活该 panel：展开它所在 dock 并切到它。已显示则幂等。
    pub(crate) fn show_panel(&mut self, panel: PanelId) {
        let Some(dock) = self.dock_hosting_mut(panel) else {
            return;
        };
        dock.collapsed = false;
        dock.stack.active = Some(panel);
    }

    /// 收起该 panel：仅当它正是其 dock 的 active 项时折叠该 dock。
    pub(crate) fn hide_panel(&mut self, panel: PanelId) {
        let Some(dock) = self.dock_hosting_mut(panel) else {
            return;
        };
        if dock.stack.active() == Some(panel) {
            dock.collapsed = true;
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

#[cfg(test)]
mod tests {
    //! Workbench 窗口 UI 状态测试。

    use crate::shell::features::panels::PanelId;
    use crate::shell::features::panels::file_tree::FileTreeState;
    use crate::shell::shared::theme;
    use crate::shell::workbench::controller::WorkbenchController;
    use crate::shell::workbench::docks::resize::{DockResizeBounds, DockResizeEvent};
    use crate::shell::workbench::state::{DockAreaId, EditorState};
    use gpui::{Pixels, point, px};

    #[test]
    fn show_and_hide_panel_should_drive_dock_visibility() {
        let mut workbench = WorkbenchController::new();
        let initial = workbench.state(
            "打开项目".to_string(),
            false,
            EditorState::default(),
            FileTreeState::default(),
            crate::shell::features::panels::search::SearchState::default(),
        );
        assert!(!initial.left_dock.is_visible());
        assert_eq!(initial.left_dock.active_panel(), None);
        assert!(!initial.right_dock.is_visible());
        assert!(!initial.bottom_dock.is_visible());

        workbench.show_panel(PanelId::FileTree);
        let after_show = workbench.state(
            "打开项目".to_string(),
            false,
            EditorState::default(),
            FileTreeState::default(),
            crate::shell::features::panels::search::SearchState::default(),
        );
        assert!(after_show.left_dock.is_visible());
        assert_eq!(after_show.left_dock.active_panel(), Some(PanelId::FileTree));

        workbench.hide_panel(PanelId::FileTree);
        let after_hide = workbench.state(
            "打开项目".to_string(),
            false,
            EditorState::default(),
            FileTreeState::default(),
            crate::shell::features::panels::search::SearchState::default(),
        );
        assert!(!after_hide.left_dock.is_visible());
    }

    #[test]
    fn show_panel_should_switch_active_panel_without_collapsing_dock() {
        let mut workbench = WorkbenchController::new();

        workbench.show_panel(PanelId::FileTree);
        workbench.show_panel(PanelId::VersionControl);

        let state = workbench.state(
            "打开项目".to_string(),
            false,
            EditorState::default(),
            FileTreeState::default(),
            crate::shell::features::panels::search::SearchState::default(),
        );
        assert!(state.left_dock.is_visible());
        assert_eq!(
            state.left_dock.active_panel(),
            Some(PanelId::VersionControl)
        );
        assert!(!workbench.is_panel_active(PanelId::FileTree));
    }

    #[test]
    fn dock_resize_should_be_clamped_to_app_width() {
        let mut workbench = WorkbenchController::new();

        workbench.handle_dock_resize(
            DockResizeEvent::Start {
                area: DockAreaId::Left,
                position: point(px(0.0), px(0.0)),
            },
            resize_bounds(px(640.0)),
        );
        workbench.handle_dock_resize(
            DockResizeEvent::Drag {
                position: point(px(660.0), px(0.0)),
            },
            resize_bounds(px(640.0)),
        );
        let state = workbench.state(
            "打开项目".to_string(),
            false,
            EditorState::default(),
            FileTreeState::default(),
            crate::shell::features::panels::search::SearchState::default(),
        );
        assert_eq!(state.left_dock.size, px(640.0) - theme::space::s12());

        workbench.handle_dock_resize(
            DockResizeEvent::Drag {
                position: point(px(-300.0), px(0.0)),
            },
            resize_bounds(px(640.0)),
        );
        let state = workbench.state(
            "打开项目".to_string(),
            false,
            EditorState::default(),
            FileTreeState::default(),
            crate::shell::features::panels::search::SearchState::default(),
        );
        assert_eq!(state.left_dock.size, theme::space::s12());
    }

    #[test]
    fn dock_resize_drag_should_follow_edge_direction() {
        let mut workbench = WorkbenchController::new();

        workbench.handle_dock_resize(
            DockResizeEvent::Start {
                area: DockAreaId::Left,
                position: point(px(100.0), px(100.0)),
            },
            resize_bounds(px(800.0)),
        );
        workbench.handle_dock_resize(
            DockResizeEvent::Drag {
                position: point(px(140.0), px(100.0)),
            },
            resize_bounds(px(800.0)),
        );
        let state = workbench.state(
            "打开项目".to_string(),
            false,
            EditorState::default(),
            FileTreeState::default(),
            crate::shell::features::panels::search::SearchState::default(),
        );
        assert_eq!(state.left_dock.size, px(280.0));

        workbench.handle_dock_resize(
            DockResizeEvent::Start {
                area: DockAreaId::Right,
                position: point(px(100.0), px(100.0)),
            },
            resize_bounds(px(800.0)),
        );
        workbench.handle_dock_resize(
            DockResizeEvent::Drag {
                position: point(px(140.0), px(100.0)),
            },
            resize_bounds(px(800.0)),
        );
        let state = workbench.state(
            "打开项目".to_string(),
            false,
            EditorState::default(),
            FileTreeState::default(),
            crate::shell::features::panels::search::SearchState::default(),
        );
        assert_eq!(state.right_dock.size, px(200.0));

        workbench.handle_dock_resize(
            DockResizeEvent::Start {
                area: DockAreaId::Bottom,
                position: point(px(100.0), px(100.0)),
            },
            resize_bounds(px(800.0)),
        );
        workbench.handle_dock_resize(
            DockResizeEvent::Drag {
                position: point(px(100.0), px(60.0)),
            },
            resize_bounds(px(800.0)),
        );
        let state = workbench.state(
            "打开项目".to_string(),
            false,
            EditorState::default(),
            FileTreeState::default(),
            crate::shell::features::panels::search::SearchState::default(),
        );
        assert_eq!(state.bottom_dock.size, px(240.0));
    }

    #[test]
    fn bottom_dock_resize_should_be_clamped_to_body_height() {
        let mut workbench = WorkbenchController::new();
        let bounds = resize_bounds(px(800.0));

        workbench.handle_dock_resize(
            DockResizeEvent::Start {
                area: DockAreaId::Bottom,
                position: point(px(0.0), px(500.0)),
            },
            bounds,
        );
        workbench.handle_dock_resize(
            DockResizeEvent::Drag {
                position: point(px(0.0), px(-100.0)),
            },
            bounds,
        );

        let state = workbench.state(
            "打开项目".to_string(),
            false,
            EditorState::default(),
            FileTreeState::default(),
            crate::shell::features::panels::search::SearchState::default(),
        );
        assert_eq!(
            state.bottom_dock.size,
            px(600.0) - px(24.0) - px(24.0) - theme::space::s12()
        );
    }

    fn resize_bounds(viewport_width: Pixels) -> DockResizeBounds {
        DockResizeBounds::from_viewport(viewport_width, px(600.0))
    }
}
