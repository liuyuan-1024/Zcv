//! 通用搜索选择器。
//! 此文件是 `zcv-picker` crate 的公共入口。
//!
//! 消费 `zcv-editor` 作为搜索输入框，供项目选择、最近项目等宿主场景复用。

mod picker_host;
mod picker_view;

pub use picker_host::PickerHost;
pub use picker_view::{Picker, PickerDelegate, picker_divider};

/// 浮层默认宽度，宿主不指定时统一取此值。
pub const PICKER_WIDTH: gpui::Pixels = gpui::px(360.0);
