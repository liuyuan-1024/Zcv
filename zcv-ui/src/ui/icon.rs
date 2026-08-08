//! 图标基础组件。
//!
//! 统一 size 和默认 color，外部可通过 builder 方法覆盖。

use gpui::{App, Component, IntoElement, Pixels, RenderOnce, Rgba, Window, prelude::*, svg};

use zcv_theme::{color, typography};

/// 图标，自带统一默认样式（字体大小灰度色 + UI 字型尺寸）。
///
/// 需要自定义 color/size 时链式调用覆盖：
/// ```ignore
/// SvgIcon::new("icons/file.svg")
///     .size(px(10.0))
///     .color(color::current(cx).icon_muted)
/// ```
pub struct SvgIcon {
    path: &'static str,
    color: Option<Rgba>,
    size: Pixels,
}

impl SvgIcon {
    pub fn new(path: &'static str) -> Self {
        Self {
            path,
            // 默认色延迟到 render（有 cx）解析
            color: None,
            size: typography::ui(),
        }
    }

    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }
}

impl IntoElement for SvgIcon {
    type Element = Component<Self>;

    fn into_element(self) -> Self::Element {
        Component::new(self)
    }
}

impl RenderOnce for SvgIcon {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        // 默认色依赖主题，只能在有 cx 的 render 中解析
        let color = self.color.unwrap_or_else(|| color::current(cx).icon);
        svg()
            .path(self.path)
            .size(self.size)
            .text_color(color)
            .flex_none()
    }
}
