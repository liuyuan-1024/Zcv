//! Shell 组件树，按手册 3.1 的 L2 / L3 / L4 分层组织。
//!
//! ```text
//! components/
//!   primitives/  L2  视觉原语（bar_frame / dock_frame / glyph / divider / placeholder）
//!   panels/      L3  业务 panel（file_tree / outline / terminal …）
//!   regions/     L4  窗口级外壳区域（top_bar / bottom_bar / dock / editor_grid …）
//! ```
//!
//! 依赖方向（手册 7.1）：L4 / L3 → L2 → L1 token；同层之间互不可见。

pub(crate) mod panels;
pub(super) mod primitives;
pub(crate) mod regions;
