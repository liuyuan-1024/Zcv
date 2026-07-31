//! 图标基础组件。
//!
//! 统一 size 和默认 color，外部可通过 builder 方法覆盖。

use gpui::{IntoElement, Pixels, Rgba, Svg, prelude::*, svg};

use zcv_theme::{color, typography};

/// 图标，自带统一默认样式（字体大小灰度色 + UI 字型尺寸）。
///
/// 需要自定义 color/size 时链式调用覆盖：
/// ```ignore
/// SvgIcon::new("icons/files/file.svg")
///     .size(px(10.0))
///     .color(color::current().icon_muted)
/// ```
pub(crate) struct SvgIcon {
    path: &'static str,
    color: Rgba,
    size: Pixels,
}

impl SvgIcon {
    pub(crate) fn new(path: &'static str) -> Self {
        Self {
            path,
            color: color::current().icon,
            size: typography::ui(),
        }
    }

    pub(crate) fn color(mut self, color: Rgba) -> Self {
        self.color = color;
        self
    }

    pub(crate) fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }
}

impl IntoElement for SvgIcon {
    type Element = Svg;

    fn into_element(self) -> Svg {
        svg().path(self.path).size(self.size).text_color(self.color)
    }
}
