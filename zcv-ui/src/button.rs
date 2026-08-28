//! `Button` —— 通用交互原语：图标 / 文字 / 图标+文字按钮。
//!
//! 统一承载点击交互（on_click / disabled / hover / tooltip）与两种视觉样式：Ghost（幽灵，默认）与 Solid（实心）。
//! 图标部分委托 `SvgIcon` 渲染。

use std::rc::Rc;

use gpui::{
    Action, App, ClickEvent, Component, ElementId, IntoElement, MouseButton, RenderOnce, Window,
    div, prelude::*,
};
use zcv_theme::{color, space, typography};

use crate::{SvgIcon, TooltipSpec};

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// 按钮视觉样式。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonStyle {
    /// 幽灵样式：悬停才显示背景，用于工具栏/状态栏等高频轻量操作。
    Ghost,
    /// 实心样式：常驻边框与背景，用于突出主操作。
    Solid,
}

#[derive(Clone)]
enum ButtonContent {
    Icon(&'static str),
    Text(String),
    IconText { icon: &'static str, text: String },
}

/// 可交互的 UI 原语。
pub struct Button {
    id: ElementId,
    content: ButtonContent,
    style: ButtonStyle,
    color: Option<gpui::Rgba>,
    tooltip: TooltipSpec,
    on_click: Option<ClickHandler>,
    disabled: bool,
}

impl Button {
    pub fn icon(id: impl Into<ElementId>, path: &'static str) -> Self {
        Self::new(id, ButtonContent::Icon(path))
    }

    pub fn text(id: impl Into<ElementId>, text: impl Into<String>) -> Self {
        Self::new(id, ButtonContent::Text(text.into()))
    }

    pub fn icon_text(
        id: impl Into<ElementId>,
        path: &'static str,
        text: impl Into<String>,
    ) -> Self {
        Self::new(
            id,
            ButtonContent::IconText {
                icon: path,
                text: text.into(),
            },
        )
    }

    fn new(id: impl Into<ElementId>, content: ButtonContent) -> Self {
        Self {
            id: id.into(),
            content,
            style: ButtonStyle::Ghost,
            color: None,
            tooltip: TooltipSpec::default(),
            on_click: None,
            disabled: false,
        }
    }

    /// 设置视觉样式。
    pub fn style(mut self, style: ButtonStyle) -> Self {
        self.style = style;
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

    /// 设置颜色（覆盖默认色）。
    pub fn color(mut self, color: gpui::Rgba) -> Self {
        self.color = Some(color);
        self
    }

    /// 设置点击回调。
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// 禁用交互，但保留图形作为可见的操作反馈。
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl IntoElement for Button {
    type Element = Component<Self>;

    fn into_element(self) -> Self::Element {
        Component::new(self)
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = *color::current(cx);
        let disabled = self.disabled;
        let on_click = self.on_click;
        let tooltip = self.tooltip;
        let style = self.style;
        let color = if disabled {
            colors.text_disabled
        } else {
            self.color.unwrap_or(colors.text)
        };

        // 容器：按样式选择视觉外壳，公共交互注入。
        let mut element = match style {
            ButtonStyle::Ghost => div().id(self.id).rounded_sm().p(space::S2),
            ButtonStyle::Solid => div()
                .id(self.id)
                .px(space::S6)
                .py(space::S6)
                .rounded_md()
                .border_1()
                .border_color(colors.border_variant)
                .bg(colors.panel_background),
        };
        // 只有可点击的按钮才显示手型光标。
        let clickable = on_click.is_some() && !disabled;
        if clickable {
            element = element
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                    cx.stop_propagation()
                });
        }
        element = element.occlude();
        if let Some(build) = tooltip.build() {
            element = element.tooltip(build);
        }
        if !disabled {
            let hover_background = match style {
                ButtonStyle::Ghost => colors.ghost_element_hover,
                ButtonStyle::Solid => colors.element_hover,
            };
            element = element.hover(move |style| style.bg(hover_background));
        }
        if !disabled && let Some(ref handler) = on_click {
            let h = Rc::clone(handler);
            element = element.on_click(move |event, window, cx| {
                h(event, window, cx);
                cx.stop_propagation();
            });
        }

        // 内容形态；图标统一经 SvgIcon 渲染。
        let content = match self.content {
            ButtonContent::Icon(path) => SvgIcon::new(path)
                .size(typography::ui())
                .color(color)
                .into_any_element(),
            ButtonContent::Text(text) => div().text_color(color).child(text).into_any_element(),
            ButtonContent::IconText { icon: path, text } => div()
                .flex()
                .flex_row()
                .items_center()
                .gap(space::S2)
                .child(SvgIcon::new(path).size(typography::ui()).color(color))
                .child(div().text_color(color).child(text))
                .into_any_element(),
        };
        element.child(content)
    }
}
