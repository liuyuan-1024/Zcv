//! 应用菜单的唯一内容定义；平台只决定呈现方式。

use gpui::{App, Menu, MenuItem, OsAction};
use zcv_actions::{
    About, CheckForUpdates, CloseTab, Copy, Cut, DeployBufferSearch, DeployProjectSearch,
    FocusLineColumn, FocusQuickAccess, MinimizeWindow, OpenLocalProject, OpenSettings, Paste,
    QuitWindow, Redo, Save, SelectAll, ToggleBottomDock, ToggleLeftDock, ToggleMaximizeWindow,
    ToggleRightDock, Undo,
};

pub(crate) fn install(cx: &App) {
    cx.set_menus([
        Menu::new("Zcv").items([
            MenuItem::action("打开设置", OpenSettings),
            MenuItem::separator(),
            MenuItem::action("退出 Zcv", QuitWindow),
        ]),
        Menu::new("文件").items([
            MenuItem::action("打开项目…", OpenLocalProject),
            MenuItem::action("切换项目…", zcv_actions::ToggleProjectPicker),
            MenuItem::separator(),
            MenuItem::action("保存", Save),
            MenuItem::action("关闭标签页", CloseTab),
        ]),
        Menu::new("编辑").items([
            MenuItem::os_action("撤销", Undo, OsAction::Undo),
            MenuItem::os_action("重做", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("剪切", Cut, OsAction::Cut),
            MenuItem::os_action("复制", Copy, OsAction::Copy),
            MenuItem::os_action("粘贴", Paste, OsAction::Paste),
            MenuItem::os_action("全选", SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("视图").items([
            MenuItem::action("切换左侧面板", ToggleLeftDock),
            MenuItem::action("切换底部面板", ToggleBottomDock),
            MenuItem::action("切换右侧面板", ToggleRightDock),
        ]),
        Menu::new("转到").items([
            MenuItem::action("快速打开…", FocusQuickAccess),
            MenuItem::action("跳转到行列…", FocusLineColumn),
            MenuItem::separator(),
            MenuItem::action("查找当前文档…", DeployBufferSearch),
            MenuItem::action("搜索项目内容…", DeployProjectSearch),
        ]),
        Menu::new("窗口").items([
            MenuItem::action("最小化", MinimizeWindow),
            MenuItem::action("最大化或恢复", ToggleMaximizeWindow),
        ]),
        Menu::new("帮助").items([
            MenuItem::action("关于 Zcv", About),
            MenuItem::action("检查更新", CheckForUpdates),
        ]),
    ]);
}

#[cfg(test)]
#[path = "test/app_menus_tests.rs"]
mod tests;
