//! 固定刻度：不随字号缩放，用于发丝线与指示条。

use gpui::{Pixels, px};

/// 1px 发丝线：边框、分隔线、缩进参考线。
pub const HAIRLINE: Pixels = px(1.0);
/// 2px 指示条：进度、选中标记等。
pub const INDICATOR: Pixels = px(2.0);
