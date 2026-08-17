//! 设计系统与基础展示组件。
//! 此文件是 `zcv-ui` crate 的公共入口。

#[path = "ui/mod.rs"]
mod ui_impl;

use ui_impl as ui;

pub use ui_impl::tree;
pub use ui_impl::{
    Checkbox, Glyph, ListItem, ScrollableHandle, Scrollbar, SvgIcon, Tab, TextInput,
    TextInputEvent, TooltipSpec, tooltip_view,
};
