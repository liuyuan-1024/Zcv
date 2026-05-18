//! Workbench 窗口 UI 状态测试。

use crate::shell::features::PanelId;
use crate::shell::workbench::controller::WorkbenchController;
use crate::shell::workbench::state::EditorState;

#[test]
fn panel_toggle_should_drive_dock_visibility_in_shell_controller() {
    let mut workbench = WorkbenchController::new();
    let initial = workbench.state("打开项目".to_string(), EditorState::default());
    assert!(initial.left_dock.is_visible());
    assert_eq!(initial.left_dock.active_panel(), Some(PanelId::FileTree));
    assert!(!initial.right_dock.is_visible());
    assert!(!initial.bottom_dock.is_visible());

    let initial_visible = initial.left_dock.is_visible();
    let file_tree_active = initial.left_dock.active_panel() == Some(PanelId::FileTree);

    workbench.toggle_panel(PanelId::FileTree);
    let after_first = workbench.state("打开项目".to_string(), EditorState::default());

    if initial_visible && file_tree_active {
        assert!(after_first.left_dock.collapsed);
    } else {
        assert!(!after_first.left_dock.collapsed);
        assert_eq!(
            after_first.left_dock.active_panel(),
            Some(PanelId::FileTree)
        );
    }

    let before = after_first.left_dock.collapsed;
    workbench.toggle_panel(PanelId::FileTree);
    let after_second = workbench.state("打开项目".to_string(), EditorState::default());
    assert_ne!(after_second.left_dock.collapsed, before);
}
