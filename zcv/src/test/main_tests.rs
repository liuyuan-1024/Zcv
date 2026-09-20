use std::ffi::OsString;
use std::path::PathBuf;

use gpui::TestAppContext;

use super::initial_project_root;

#[test]
fn startup_uses_explicit_command_line_project() {
    assert_eq!(
        initial_project_root(
            [OsString::from("zcv"), OsString::from("/project")].into_iter(),
            Some(PathBuf::from("/recent")),
        ),
        Some(PathBuf::from("/project"))
    );
}

#[test]
fn startup_without_path_uses_recent_project_or_none() {
    assert_eq!(
        initial_project_root(
            [OsString::from("zcv")].into_iter(),
            Some(PathBuf::from("/recent")),
        ),
        Some(PathBuf::from("/recent"))
    );
    assert_eq!(
        initial_project_root([OsString::from("zcv")].into_iter(), None),
        None
    );
}

/// Panel 键盘命令使用单一带参 Action，不为每个 Panel 声明类型。
#[gpui::test]
fn panel_keyboard_action_uses_stable_panel_id(cx: &mut TestAppContext) {
    cx.update(|cx| {
        for action in ["ToggleLeftDock", "ToggleBottomDock", "ToggleRightDock"] {
            assert!(cx.build_action(&format!("dock::{action}"), None).is_ok());
        }
        let action = cx
            .build_action(
                "dock::FocusOrHidePanel",
                Some(serde_json::json!({ "panel": "project-tree" })),
            )
            .expect("带稳定 Panel ID 的通用 Action 应能构建");
        assert_eq!(action.name(), "dock::FocusOrHidePanel");
    });
}
