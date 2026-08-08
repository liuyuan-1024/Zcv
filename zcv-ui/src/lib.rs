//! zcv-ui —— 设计系统：基础展示组件。

mod ui;

pub use ui::tree;
pub use ui::tree::{git_status_color, render_row_base, selection_border};
pub use ui::{Checkbox, Glyph, ListItem, SvgIcon, Tab, TooltipSpec, tooltip_view};
