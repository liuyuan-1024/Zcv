//! Workbench 内稳定 element id。
//!
//! 这些 id 是 bar 与 overlay 锚点之间的协议：bar 渲染时写入 element，
//! overlay 解释动作时用同一份 id 选择锚点。

pub(crate) const TOP_BAR_WORKSPACE: &str = "top-bar.workspace";
pub(crate) const BOTTOM_BAR_LANGUAGE_SERVER: &str = "bottom-bar.language_server";
