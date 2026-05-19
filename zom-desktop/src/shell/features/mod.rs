//! L3 功能切片 —— UI、元数据与局部行为放在同一个功能目录。
//!
//! 第一版骨架里多数功能还是 panel 占位，但目录已经按功能归拢：接入真实数据源时，
//! 在各自目录内补充 `view.rs` / `state.rs` / `actions.rs` 等文件。
//!
//! Dock 归属由对应 dock 模块声明，本目录只提供功能本身。

pub(crate) mod debug;
pub(crate) mod file_tree;
pub(crate) mod keyboard_shortcuts;
pub(crate) mod language_servers;
pub(crate) mod outline;
mod panel;
pub(crate) mod project_picker;
pub(crate) mod project_search;
pub(crate) mod terminal;
pub(crate) mod version_control;

pub(crate) use panel::PanelId;
