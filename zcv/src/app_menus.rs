//! 应用菜单的唯一内容定义；平台只决定呈现方式。

use gpui::{App, Menu, MenuItem};
use zcv_actions::{About, CheckForUpdates};

pub(crate) fn install(cx: &App) {
    cx.set_menus([Menu::new("帮助").items([
        MenuItem::action("关于 Zcv", About),
        MenuItem::action("检查更新", CheckForUpdates),
    ])]);
}

#[cfg(test)]
#[path = "test/app_menus_tests.rs"]
mod tests;
