//! 图标展示组件。
//!
//! 统一 size 和默认 color，外部可通过 builder 方法覆盖；
//! 需要悬停提示时设置 id 与 label，其余场景无需 id。

use gpui::{
    Action, App, ElementId, IntoElement, Pixels, RenderOnce, Rgba, SharedString, ViewElement,
    Window, div, prelude::*, svg,
};
use zcv_theme::{color, typography};

use crate::TooltipSpec;

/// 图标，自带统一默认样式（主题色 + UI 字型尺寸）。
///
/// 需要自定义 color/size 时链式调用覆盖：
/// ```ignore
/// SvgIcon::new("icons/file.svg")
///     .size(px(10.0))
///     .color(color::current(cx).icon_muted)
/// ```
/// 需要悬停提示（label/shortcut）时先设置 id：
/// ```ignore
/// SvgIcon::new("icons/check.svg").id(("head", index)).label("当前分支")
/// ```
pub struct SvgIcon {
    id: Option<ElementId>,
    path: SharedString,
    color: Option<Rgba>,
    size: Pixels,
    tooltip: TooltipSpec,
}

impl SvgIcon {
    pub fn new(path: impl Into<SharedString>) -> Self {
        Self {
            id: None,
            path: path.into(),
            // 默认色延迟到 render（有 cx）解析
            color: None,
            size: typography::ui_size(),
            tooltip: TooltipSpec::default(),
        }
    }

    /// 设置元素标识；悬停提示依赖 stateful 元素，设置 tooltip 前必须提供。
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn color(mut self, color: Rgba) -> Self {
        self.color = Some(color);
        self
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    /// 设置 tooltip 标签文字。
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.tooltip = TooltipSpec::new(label);
        self
    }

    /// 从当前 keymap 中获取 action 对应的快捷键并设为提示。
    pub fn shortcut(mut self, action: &dyn Action, cx: &App) -> Self {
        self.tooltip = self.tooltip.with_action(action, cx);
        self
    }
}

impl IntoElement for SvgIcon {
    type Element = ViewElement<Self>;

    fn into_element(self) -> Self::Element {
        ViewElement::new(self)
    }
}

impl RenderOnce for SvgIcon {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        // 默认色依赖主题，只能在有 cx 的 render 中解析
        let color = self.color.unwrap_or_else(|| color::current(cx).icon);
        let icon = svg()
            .path(self.path)
            .size(self.size)
            .text_color(color)
            .flex_none();

        match self.id {
            Some(id) => {
                let mut element = div().id(id).child(icon);
                if let Some(build) = self.tooltip.build() {
                    element = element.tooltip(build);
                }
                element.into_any_element()
            }
            None => div().child(icon).into_any_element(),
        }
    }
}
