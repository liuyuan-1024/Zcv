//! zcv-ui —— 设计系统：基础展示组件与通用搜索-选择器。

mod ui;

pub use ui::tree;
pub use ui::tree::{git_status_color, render_row_base, selection_border};
pub use ui::{
    Checkbox, Glyph, ListItem, Picker, PickerDelegate, SvgIcon, Tab, list_item_two_line,
    picker_divider, tooltip_for_action, tooltip_view,
};
