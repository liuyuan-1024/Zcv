//! Workbench 窗口 UI 状态测试。

use crate::shell::features::PanelId;
use crate::shell::features::file_tree::FileTreeState;
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
    );
    assert!(after_show.left_dock.is_visible());
    assert_eq!(after_show.left_dock.active_panel(), Some(PanelId::FileTree));

    workbench.hide_panel(PanelId::FileTree);
    let after_hide = workbench.state(
        "打开项目".to_string(),
        false,
        EditorState::default(),
        FileTreeState::default(),
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
    );
    assert_eq!(
        state.bottom_dock.size,
        px(600.0) - px(24.0) - px(24.0) - theme::space::s12()
    );
}

fn resize_bounds(viewport_width: Pixels) -> DockResizeBounds {
    DockResizeBounds::from_viewport(viewport_width, px(600.0))
}
