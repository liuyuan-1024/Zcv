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
mod tree_disclosure;

pub use autoscroll::drag_autoscroll_delta;
pub use button::{Button, ButtonSize, ButtonStyle};
pub use button_like::ButtonLike;
pub use checkbox::Checkbox;
pub use confirm::{ConfirmAnswer, ConfirmOverlay};
pub use icon::SvgIcon;
pub use input::{EDITOR_FACTORY, ErasedEditor, ErasedEditorEvent};
pub use list_item::ListItem;
pub use replace_input::ReplaceInput;
pub use scrollbar::{ScrollableHandle, Scrollbar, thumb_geometry};
pub use search_input::{MatchOption, MatchOptions, SearchInput, search_box};
pub use tab::Tab;
pub use tooltip::{ShortcutResolver, TooltipSpec};
pub use tree::{
    AutoFoldDir, RowClickAction, TreeNodeRow, TreeRow, TreeRowFrame, TreeState, auto_fold_dirs,
    row_click_action, selection_border, tree_row_height, tree_row_label,
};
pub use tree_disclosure::TreeDisclosure;
