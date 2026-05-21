//! 项目切换功能。

mod overlay;

pub(crate) use overlay::render;

/// 功能显示名（顶栏 tooltip 与 overlay 标题共用）—— 名字归功能自己持有。
pub(crate) const FEATURE_TITLE: &str = "切换项目";
