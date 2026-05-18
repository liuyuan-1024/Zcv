//! L3 业务 panel —— 可挂载到任一 Dock 的内容组件。
//!
//! 第一版骨架：每个 panel 一个目录、内含一个 `mod.rs`，渲染 `panel_placeholder`
//! 占位文案。各 panel 自给自足，互不可见；接入真实数据源时在自己目录内补充
//! `view.rs` / `state.rs` / `logic.rs` 等子文件（手册 3.3）。
//!
//! Dock 归属由 `app::default_layout` 决定，本目录不感知。

pub(crate) mod debug;
pub(crate) mod file_tree;
mod host;
pub(crate) mod keyboard_shortcuts;
pub(crate) mod outline;
pub(crate) mod project_search;
pub(crate) mod terminal;
pub(crate) mod version_control;

pub(crate) use host::PanelHost;
