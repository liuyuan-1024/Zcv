//! 语言服务器状态功能。

mod overlay;

pub(crate) use overlay::render;

/// 该功能在底栏的图标 —— 视觉身份归功能自己持有，承载它的 bar 不重新描述。
pub(crate) const BAR_ICON: &str = "icons/bottom_bar/language_server.svg";
/// 功能显示名（底栏 tooltip 与 overlay 标题共用）。
pub(crate) const FEATURE_TITLE: &str = "语言服务器";
