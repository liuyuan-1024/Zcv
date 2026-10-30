use std::{ffi::OsString, path::PathBuf};

use gpui::{AppContext, TestAppContext};
use zcv_language::LanguageRegistry;
use zcv_settings::{
    GlobalSettingsErrorReporter, SettingsErrorReporter, SettingsStore, UserSettings,
};
use zcv_workspace::Workspace;

use super::{InitialOpen, initial_open, open_initial};

#[test]
fn startup_uses_explicit_command_line_project() {
    assert_eq!(
        initial_open(
            [OsString::from("zcv"), OsString::from("/project")].into_iter(),
            Some(PathBuf::from("/recent")),
        ),
        InitialOpen::Project(PathBuf::from("/project"))
    );
}

#[test]
fn startup_without_path_uses_recent_project_or_none() {
    assert_eq!(
        initial_open(
            [OsString::from("zcv")].into_iter(),
            Some(PathBuf::from("/recent")),
        ),
        InitialOpen::Project(PathBuf::from("/recent"))
    );
    assert_eq!(
        initial_open([OsString::from("zcv")].into_iter(), None),
        InitialOpen::Empty
    );
}

#[test]
fn startup_file_argument_opens_as_document() {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let file = directory.path().join("页面.html");
    std::fs::write(&file, "<h1>你好</h1>").expect("应创建 HTML 文件");
    assert_eq!(
        initial_open(
            [OsString::from("zcv"), file.as_os_str().to_owned()].into_iter(),
            Some(PathBuf::from("/recent")),
        ),
        InitialOpen::File(file)
    );
}

#[gpui::test]
fn startup_file_opens_in_empty_workspace(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let file = directory.path().join("页面.html");
    std::fs::write(&file, "<h1>你好</h1>").expect("应创建 HTML 文件");
    let canonical_file = zcv_path::AbsolutePathBuf::canonicalize(&file)
        .expect("文件应可规范化")
        .as_path()
        .to_path_buf();
    let languages = std::sync::Arc::new(LanguageRegistry::new());
    cx.update(|cx| {
        let reporter = cx.new(|_| SettingsErrorReporter::default());
        cx.set_global(GlobalSettingsErrorReporter(reporter));
        cx.set_global(SettingsStore::new(UserSettings::default()));
        zcv_editor::init(cx, languages.clone());
        crate::app_menus::install(cx);
        open_initial(InitialOpen::File(file), languages, cx);
    });
    cx.run_until_parked();

    cx.update(|cx| {
        let windows = cx.windows();
        assert_eq!(windows.len(), 1);
        windows[0]
            .update(cx, |_, window, cx| {
                let workspace = window.root::<Workspace>().flatten().unwrap();
                let workspace = workspace.read(cx);
                assert!(workspace.project().read(cx).root().is_none());
                assert_eq!(
                    workspace
                        .pane()
                        .read(cx)
                        .active_item()
                        .and_then(|item| item.item_path(cx)),
                    Some(canonical_file)
                );
            })
            .unwrap();
    });
}

#[gpui::test]
fn incoming_file_reuses_existing_window(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let file = directory.path().join("第二个文件.md");
    std::fs::write(&file, "内容").expect("应创建文件");
    let languages = std::sync::Arc::new(LanguageRegistry::new());
    cx.update(|cx| {
        let reporter = cx.new(|_| SettingsErrorReporter::default());
        cx.set_global(GlobalSettingsErrorReporter(reporter));
        cx.set_global(SettingsStore::new(UserSettings::default()));
        zcv_editor::init(cx, languages.clone());
        crate::app_menus::install(cx);
        open_initial(InitialOpen::Empty, languages.clone(), cx);
        open_initial(InitialOpen::File(file.clone()), languages, cx);
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let windows = cx.windows();
        assert_eq!(windows.len(), 1);
        windows[0]
            .update(cx, |_, window, cx| {
                let workspace = window.root::<Workspace>().flatten().unwrap();
                assert_eq!(
                    workspace
                        .read(cx)
                        .pane()
                        .read(cx)
                        .active_item()
                        .and_then(|item| item.item_path(cx))
                        .and_then(|path| path.file_name().map(|name| name.to_owned())),
                    file.file_name().map(|name| name.to_owned())
                );
            })
            .unwrap();
    });
}

#[gpui::test]
fn closed_last_window_can_open_a_workspace_again(cx: &mut TestAppContext) {
    let languages = std::sync::Arc::new(LanguageRegistry::new());
    cx.update(|cx| {
        let reporter = cx.new(|_| SettingsErrorReporter::default());
        cx.set_global(GlobalSettingsErrorReporter(reporter));
        cx.set_global(SettingsStore::new(UserSettings::default()));
        zcv_editor::init(cx, languages.clone());
        crate::app_menus::install(cx);
        open_initial(InitialOpen::Empty, languages.clone(), cx);
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
        open_initial(InitialOpen::Empty, languages, cx);
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
