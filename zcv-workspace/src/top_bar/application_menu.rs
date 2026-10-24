//! 顶栏应用菜单的统一呈现入口；菜单内容由应用装配层安装到 GPUI。

mod platform;

pub(super) use platform::ApplicationMenu;
