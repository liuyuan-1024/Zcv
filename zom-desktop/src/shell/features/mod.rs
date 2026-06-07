//! L3 功能切片 —— UI、元数据与局部行为放在同一个功能目录。
//!
//! Dock panel 统一放在 `features::panels`，非 panel 的 bar / surface 入口保留在
//! 本层功能目录。

pub(crate) mod diagnostics;
pub(crate) mod language_servers;
pub(crate) mod panels;
pub(crate) mod project_picker;
pub(crate) mod search;
pub(crate) mod settings;
