//! 诊断功能。
//!
//! 第一版只占位：功能身份（图标、名字）归本模块所有，承载它的底栏只引用。
//! 问题面板 UI 待后续开发，届时在本目录补 `view.rs` / `state.rs` 等文件。

/// 该功能在底栏的图标。
pub(crate) const BAR_ICON: &str = "icons/bottom_bar/diagnostics.svg";
/// 功能显示名（底栏 tooltip 等 UI 共用）。
pub(crate) const FEATURE_TITLE: &str = "诊断";
