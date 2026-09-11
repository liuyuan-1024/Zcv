//! 设计系统与基础展示组件。
//! 此文件是 `zcv-ui` crate 的公共入口。

mod autoscroll;
mod button;
mod button_like;
mod checkbox;
mod confirm;
mod icon;
mod input;
mod input_shell;
mod list_item;
mod replace_input;
mod scrollbar;
mod search_input;
mod tab;
mod tooltip;
mod tree;

pub use autoscroll::drag_autoscroll_delta;
pub use button::{Button, ButtonSize, ButtonStyle};
pub use button_like::ButtonLike;
pub use checkbox::Checkbox;
pub use confirm::{ConfirmAnswer, ConfirmOverlay};
pub use icon::SvgIcon;
pub use input::{EDITOR_FACTORY, ErasedEditor, ErasedEditorEvent};
pub use list_item::ListItem;
pub use replace_input::ReplaceInput;
pub use scrollbar::{MIN_THUMB_SIZE, ScrollableHandle, Scrollbar};
pub use search_input::{MatchOption, MatchOptions, SearchInput};
pub use tab::Tab;
pub use tooltip::TooltipSpec;
pub use tree::{
    RowClickAction, TreeNodeRow, TreeRow, TreeRowFrame, TreeState, row_click_action,
    selection_border, tree_row_height,
};
