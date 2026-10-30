use gpui::{OwnedMenuItem, TestAppContext};

use super::*;

#[gpui::test]
fn application_menus_expose_existing_editor_commands(cx: &mut TestAppContext) {
    cx.update(|cx| {
        install(cx);
        let menus = cx.get_menus().expect("应用菜单应已安装");
        let names = menus
            .iter()
            .map(|menu| menu.name.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(names, ["Zcv", "文件", "编辑", "视图", "转到", "窗口"]);

        let action_names = |menu_index: usize| {
            menus[menu_index]
                .items
                .iter()
                .filter_map(|item| match item {
                    OwnedMenuItem::Action { action, .. } => Some(action.name()),
                    OwnedMenuItem::Separator => None,
                    _ => panic!("菜单只应包含操作和分隔线"),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            action_names(0),
            [
                "app::About",
                "app::CheckForUpdates",
                "workspace::OpenSettings",
                "window_controls::QuitWindow"
            ]
        );
        assert_eq!(
            action_names(1),
            [
                "workspace::OpenLocalFile",
                "workspace::OpenLocalDirectory",
                "recent_projects::ToggleRecentProjects",
                "workspace::Save",
                "pane::CloseTab",
            ]
        );
        assert_eq!(
            menus[1]
                .items
                .iter()
                .filter_map(|item| match item {
                    OwnedMenuItem::Action { name, .. } => Some(name.as_str()),
                    _ => None,
                })
                .take(2)
                .collect::<Vec<_>>(),
            ["打开本地文件…", "打开本地目录…"]
        );
        assert_eq!(
            action_names(2),
            [
                "editor::Undo",
                "editor::Redo",
                "editor::Cut",
                "editor::Copy",
                "editor::Paste",
                "editor::SelectAll"
            ]
        );
    });
}
