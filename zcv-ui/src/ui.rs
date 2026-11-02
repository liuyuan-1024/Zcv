//! 设计系统与基础展示组件。
//! 此文件是 `zcv-ui` crate 的公共入口。

mod autoscroll;
mod blink;
mod button;
mod button_like;
mod checkbox;
mod confirm;
mod contrast;
mod icon;
mod input;
mod input_shell;
mod list_item;
mod popover_host;
mod replace_input;
mod scrollbar;
mod search_input;
mod selection_shape;
mod tab;
mod tooltip;
mod tree;
mod tree_disclosure;

pub use autoscroll::{AUTOSCROLL_INTERVAL, DragAutoscroll};
pub use blink::BlinkManager;
pub use button::{Button, ButtonSize, ButtonStyle};
pub use button_like::ButtonLike;
pub use checkbox::Checkbox;
pub use confirm::{ConfirmAnswer, ConfirmOverlay};
pub use contrast::ensure_minimum_contrast;
pub use icon::SvgIcon;
pub use input::{EDITOR_FACTORY, ErasedEditor, ErasedEditorEvent};
pub use list_item::ListItem;
pub use popover_host::{PopoverDismissHandler, PopoverHost, PopoverLayer};
pub use replace_input::ReplaceInput;
pub use scrollbar::{ScrollableHandle, Scrollbar, thumb_geometry};
pub use search_input::{MatchOption, MatchOptions, SearchInput, search_box};
pub use selection_shape::{
    ALL_STRAIGHT, BOTTOM_LEFT, BOTTOM_RIGHT, Corner, CornerStyle, SelectionContourLine,
    SelectionSegmentShape, TOP_LEFT, TOP_RIGHT, paint_selection_segment, selection_corner_radius,
    selection_line_end_overshoot, selection_segment_shapes,
};
pub use tab::Tab;
pub use tooltip::{ShortcutResolver, TooltipSpec};
pub use tree::{
    AutoFoldDir, RowClickAction, TreeNodeRow, TreeRow, TreeRowFrame, TreeState, auto_fold_dirs,
    row_click_action, selection_border, tree_row_height, tree_row_label,
};
pub use tree_disclosure::TreeDisclosure;
