//! L4 窗口级外壳区域 —— 由 `app` 组合根装配，每窗口一份。
//!
//! ```text
//! workbench_frame/   顶层装配：TopBar + Body + BottomBar + Overlay/Bubble portal
//!   top_bar/         窗口级顶栏（窗控圆点 / workspace 入口 / 设置）
//!   bottom_bar/      窗口级底栏（panel.toggle 槽 + 状态指示）
//!   center_column/   中列容器：EditorGrid + BottomDock
//!     editor_grid/   主编辑区
//!     bottom_dock/   中列底部停靠区
//!   left_dock/       左停靠区
//!   right_dock/      右停靠区
//!   overlay_layer/   悬浮层 portal（z-index 20）
//!   bubble_layer/    气泡层 portal（z-index 30）
//! ```
//!
//! 区域之间不互相 `use`：跨区域协作只走共享 entity 与 `WorkbenchState`（手册 19.10）。
//! 唯一例外是 `workbench_frame`——它本身就是顶层装配，必须 use 所有兄弟区域。

pub(crate) mod bottom_bar;
pub(crate) mod bottom_dock;
pub(crate) mod bubble_layer;
pub(crate) mod center_column;
pub(crate) mod editor_grid;
pub(crate) mod left_dock;
pub(crate) mod overlay_layer;
pub(crate) mod right_dock;
pub(crate) mod top_bar;
pub(crate) mod workbench_frame;
