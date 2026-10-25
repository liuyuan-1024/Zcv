use std::{ffi::OsString, path::PathBuf};

use gpui::{AppContext, TestAppContext};
use zcv_language::LanguageRegistry;
use zcv_settings::{
    GlobalSettingsErrorReporter, SettingsErrorReporter, SettingsStore, UserSettings,
};

use super::{initial_project_root, open_workspace_or_empty};

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

#[gpui::test]
fn closed_last_window_can_open_a_workspace_again(cx: &mut TestAppContext) {
    let languages = std::sync::Arc::new(LanguageRegistry::new());
    cx.update(|cx| {
        let reporter = cx.new(|_| SettingsErrorReporter::default());
        cx.set_global(GlobalSettingsErrorReporter(reporter));
        cx.set_global(SettingsStore::new(UserSettings::default()));
        zcv_editor::init(cx, languages.clone());
        open_workspace_or_empty(None, languages.clone(), cx);
        assert_eq!(cx.windows().len(), 1);
    });

    let window = cx.update(|cx| cx.windows().pop().expect("应有一个工作区窗口"));
    cx.update(|cx| {
        window
            .update(cx, |_, window, _| window.remove_window())
            .expect("应能关闭工作区窗口");
    });
    cx.run_until_parked();

    cx.update(|cx| {
        assert!(cx.windows().is_empty());
        open_workspace_or_empty(None, languages, cx);
        assert_eq!(cx.windows().len(), 1);
    });
}

/// Panel 键盘命令使用单一带参 Action，不为每个 Panel 声明类型。
#[gpui::test]
fn panel_keyboard_action_uses_stable_panel_id(cx: &mut TestAppContext) {
    cx.update(|cx| {
        for action in ["ToggleLeftDock", "ToggleBottomDock"] {
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
