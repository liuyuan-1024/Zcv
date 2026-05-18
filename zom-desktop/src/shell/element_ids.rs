//! Shell 级稳定 element id。
//!
//! 这些 id 被多个 shell 子系统共同引用：组件渲染时写入 element，overlay
//! anchor resolver 再用同一份 id 选择锚点。

pub(crate) const TOP_BAR_WORKSPACE: &str = "top-bar.workspace";
pub(crate) const BOTTOM_BAR_LANGUAGE_SERVER: &str = "bottom-bar.language_server";
