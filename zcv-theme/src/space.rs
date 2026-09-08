//! 间距 token：全项目统一的间距与尺寸刻度。
//!
//! 常量以像素值命名（`S6` = 6px），用于 `gap`、尺寸计算、结构体字段和函数参数等场景。
//! 统一使用这些 token 可以避免重复硬编码，并保证相同间距在不同组件中的视觉一致性。
//!
//! 元素链中的固定宽高、边距和内边距优先使用本模块的间距 token；不要使用 GPUI 的数字快捷 API，避免把 rem 刻度误当成像素值。

use gpui::{Pixels, px};

/// 1px 发丝线：用于分隔线和缩进参考线。
pub const S1: Pixels = px(1.0);
/// 2px 极小间距：用于图标、标签等行内元素的紧凑间隔。
pub const S2: Pixels = px(2.0);
/// 4px 小间距：用于相邻小元素之间的间隔和紧凑布局。
pub const S4: Pixels = px(4.0);
/// 6px 标准间距：组件内部间隔和常规留白的主要刻度。
pub const S6: Pixels = px(6.0);
/// 8px 中等间距：用于分组间隔和区块留白。
pub const S8: Pixels = px(8.0);
/// 10px 中大间距：用于浮层等内容区域的留白。
pub const S10: Pixels = px(10.0);
/// 12px 大间距：用于对话框、面板等容器的留白。
pub const S12: Pixels = px(12.0);
/// 16px 结构尺寸基准：用于拖拽把手、dock 等元素的最小宽高。
pub const S16: Pixels = px(16.0);
/// 32px 大尺寸基准：用于内容区域的最小宽度等结构尺寸。
pub const S32: Pixels = px(32.0);
