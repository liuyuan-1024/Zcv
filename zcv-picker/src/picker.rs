//! 通用搜索选择器。
//! 此文件是 `zcv-picker` crate 的公共入口。
//!
//! 消费 `zcv-editor` 作为搜索输入框，供项目选择、最近项目等宿主场景复用。

mod picker_view;

use gpui::{App, Entity, Pixels, Window, px};
pub use picker_view::{Picker, PickerDelegate, picker_divider};
use zcv_ui::PopoverHost;

/// 选择器打开时聚焦搜索输入，浮层范围仍由选择器自身标识。
pub fn toggle_picker<D: PickerDelegate>(
    host: &mut PopoverHost,
    picker: &Entity<Picker<D>>,
    window: &mut Window,
    cx: &mut App,
) {
    let picker = picker.read(cx);
    host.toggle(
        picker.focus_handle(),
        picker.search_input().focus_handle(cx),
        window,
        cx,
    );
}

/// 浮层默认宽度，宿主不指定时统一取此值。
pub const PICKER_WIDTH: Pixels = px(360.0);

/// 浮层高度上限（内容超出时列表滚动）。
pub(crate) const PICKER_MAX_HEIGHT: Pixels = px(420.0);
