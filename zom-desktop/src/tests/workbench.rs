//! Workbench 窗口 UI 状态测试。

use crate::shell::features::PanelId;
use crate::shell::features::file_tree::FileTreeState;
use crate::shell::shared::theme;
use crate::shell::workbench::controller::WorkbenchController;
use crate::shell::workbench::dock_resize::{DockResizeBounds, DockResizeEvent};
use crate::shell::workbench::state::{DockAreaId, EditorState};
use gpui::{Pixels, point, px};

#[test]
fn panel_toggle_should_drive_dock_visibility_in_shell_controller() {
    let mut workbench = WorkbenchController::new();
    let initial = workbench.state(
        "打开项目".to_string(),
        false,
        EditorState::default(),
        FileTreeState::default(),
    );
    assert!(!initial.left_dock.is_visible());
    assert_eq!(initial.left_dock.active_panel(), None);
    assert!(!initial.right_dock.is_visible());
    assert!(!initial.bottom_dock.is_visible());

    workbench.toggle_panel(PanelId::FileTree);
    let after_first = workbench.state(
        "打开项目".to_string(),
        false,
        EditorState::default(),
        FileTreeState::default(),
    );

    assert!(after_first.left_dock.is_visible());
    assert_eq!(
        after_first.left_dock.active_panel(),
        Some(PanelId::FileTree)
    );

    let before = after_first.left_dock.collapsed;
    workbench.toggle_panel(PanelId::FileTree);
    let after_second = workbench.state(
        "打开项目".to_string(),
        false,
        EditorState::default(),
        FileTreeState::default(),
    );
    assert_ne!(after_second.left_dock.collapsed, before);
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
    );
    assert_eq!(state.bottom_dock.size, px(240.0));
}

#[test]
fn bottom_dock_resize_should_be_clamped_to_body_height() {
    let mut workbench = WorkbenchController::new();
    let bounds = DockResizeBounds::from_viewport(px(800.0));

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
    );
    assert_eq!(
        state.bottom_dock.size,
        px(600.0) - px(24.0) - px(24.0) - theme::space::s12()
    );
}

fn resize_bounds(viewport_width: Pixels) -> DockResizeBounds {
    DockResizeBounds {
        width: viewport_width,
    }
}
