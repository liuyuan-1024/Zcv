//! 结构刻度：随窗口 UI 字号缩放。
//!
//! 以 rem 形式的 [`DefiniteLength`] 表达，GPUI 在排版时按 `window.rem_size()` 解析，因此修改字号与每窗口缩放都会自动跟随。
//! 像素值以默认 UI 字号（[`DEFAULT_UI_SIZE`]）为基准，需要具体像素时用 [`to_pixels`] / [`to_pixels_at`]。
//!
//! 不随字号变化的发丝线与指示条在 [`fixed`]。

use gpui::{AbsoluteLength, DefiniteLength, Pixels, Rems, Window, px};

// 默认 UI 字号来自内置设置文件；由 build.rs 在构建期读取生成，避免重复硬编码。
include!(concat!(env!("OUT_DIR"), "/default_ui_size.rs"));

/// 按「默认字号下的像素值」定义结构长度。
pub const fn structural(pixels_at_default: f32) -> DefiniteLength {
    DefiniteLength::Absolute(AbsoluteLength::Rems(Rems(
        pixels_at_default / DEFAULT_UI_SIZE,
    )))
}

/// 把结构长度按给定 rem 尺寸解析为像素。
pub fn to_pixels_at(length: DefiniteLength, rem_size: Pixels) -> Pixels {
    length.to_pixels(AbsoluteLength::Pixels(px(0.0)), rem_size)
}

/// 把结构长度解析为当前窗口下的像素值。
///
/// 只在需要具体像素（度量计算、滚动、命中测试）时使用；布局直接传结构长度即可。
pub fn to_pixels(length: DefiniteLength, window: &Window) -> Pixels {
    to_pixels_at(length, window.rem_size())
}

/// 2px 极小结构间距：图标、标签等行内元素的紧凑间隔。
pub const S2: DefiniteLength = structural(2.0);
/// 4px 小结构间距：相邻小元素之间的间隔和紧凑布局。
pub const S4: DefiniteLength = structural(4.0);
/// 6px 标准结构间距：组件内部间隔和常规留白的主要刻度。
pub const S6: DefiniteLength = structural(6.0);
/// 8px 中等结构间距：分组间隔和区块留白。
pub const S8: DefiniteLength = structural(8.0);
/// 10px 中大结构间距：浮层等内容区域的留白。
pub const S10: DefiniteLength = structural(10.0);
/// 12px 大结构间距：对话框、面板等容器的留白。
pub const S12: DefiniteLength = structural(12.0);
/// 16px 结构尺寸基准：拖拽把手、dock 等元素的最小宽高。
pub const S16: DefiniteLength = structural(16.0);
/// 32px 大结构尺寸：内容区域的最小宽度等。
pub const S32: DefiniteLength = structural(32.0);
