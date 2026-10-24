//! macOS 的应用菜单由 GPUI 交给系统菜单栏呈现，顶栏不再绘制一份菜单。

use gpui::{AnyElement, App, Context};
use zcv_ui::PopoverLayer;

use super::super::super::TopBar;

pub(in crate::top_bar) struct ApplicationMenu;

impl ApplicationMenu {
    pub(in crate::top_bar) fn new(_layer: PopoverLayer, cx: &mut Context<TopBar>) -> Self {
        cx.get_menus().expect("应用菜单必须先于工作区安装");
        Self
    }

    pub(in crate::top_bar) fn leading_slot(&self) -> Option<AnyElement> {
        None
    }

    pub(in crate::top_bar) fn is_expanded(&self, _cx: &App) -> bool {
        false
    }
}
