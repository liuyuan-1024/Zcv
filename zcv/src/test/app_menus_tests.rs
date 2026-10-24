use gpui::{OwnedMenuItem, TestAppContext};

use super::*;

#[gpui::test]
fn help_menu_contains_version_and_update_actions(cx: &mut TestAppContext) {
    cx.update(|cx| {
        install(cx);
        let menus = cx.get_menus().expect("应用菜单应已安装");
        assert_eq!(menus.len(), 1);
        assert_eq!(menus[0].name.as_ref(), "帮助");
        let actions = menus[0]
            .items
            .iter()
            .map(|item| match item {
                OwnedMenuItem::Action { action, .. } => action.name(),
                _ => panic!("帮助菜单只应包含操作"),
            })
            .collect::<Vec<_>>();
        assert_eq!(actions, ["app::About", "app::CheckForUpdates"]);
    });
}
