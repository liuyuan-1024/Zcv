//! 间距 token：全项目统一的 padding / gap / 尺寸刻度。
//!
//! 常量以像素值命名（`S6` = 6px），组件按刻度取用而非硬编码数值，保证同值在所有组件上视觉一致。

use gpui::{Pixels, px};

/// 1px 发丝线：分隔线、缩进参考线（用 `.w`/`.h` 画细线）。
pub const S1: Pixels = px(1.0);
/// 2px 极小间距：图标与标签等行内元素的紧凑间隔、微小内边距。
pub const S2: Pixels = px(2.0);
/// 4px 小间距：紧凑垂直内边距、相邻小元素间隔。
pub const S4: Pixels = px(4.0);
/// 6px 标准间距：组件内 gap 与内边距的主力刻度（行内元素间隔、常规 padding）。
pub const S6: Pixels = px(6.0);
/// 8px 中等间距：分组内 gap、区块内边距。
pub const S8: Pixels = px(8.0);
/// 10px 中大内边距：浮层（toast 等）的内容留白。
pub const S10: Pixels = px(10.0);
/// 12px 面板内边距：对话框、面板等较大容器的留白。
pub const S12: Pixels = px(12.0);
/// 16px 最小尺寸基准：拖拽把手、dock 等的最小宽/高。
pub const S16: Pixels = px(16.0);
