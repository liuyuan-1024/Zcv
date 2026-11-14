//! `ButtonLike` —— 为自定义内容提供统一鼠标交互的轻量容器。
//!
//! 适用于内容布局不属于标准图文按钮、但仍需要悬停提示或鼠标操作的场景。

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, CursorStyle, DefiniteLength, Div, ElementId, IntoElement,
    MouseButton, MouseUpEvent, ParentElement, RenderOnce, Stateful, ViewElement, Window, div,
    prelude::*,
};
use zcv_theme::{color, scale};

use crate::TooltipSpec;

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

fn cursor_for_state(disabled: bool, has_click_handler: bool) -> Option<CursorStyle> {
    if disabled {
        Some(CursorStyle::OperationNotAllowed)
    } else if has_click_handler {
        Some(CursorStyle::PointingHand)
    } else {
        None
    }
}

/// 按钮视觉样式。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonStyle {
    /// 幽灵样式：悬停才显示背景，用于工具栏/状态栏等高频轻量操作。
    Ghost,
    /// 实心样式：常驻边框与背景，用于突出主操作。
    Solid,
}

type RightClickHandler = Rc<dyn Fn(&MouseUpEvent, &mut Window, &mut App)>;

/// 自定义内容的鼠标交互容器。
pub struct ButtonLike {
    id: ElementId,
    flex_grow: bool,
    padding: DefiniteLength,
    style: ButtonStyle,
    disabled: bool,
    occlude: bool,
    tooltip: TooltipSpec,
    on_click: Option<ClickHandler>,
    on_right_click: Option<RightClickHandler>,
    children: Vec<AnyElement>,
}

impl ButtonLike {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            flex_grow: false,
            padding: scale::S4,
            style: ButtonStyle::Ghost,
            disabled: false,
            occlude: false,
            tooltip: TooltipSpec::default(),
            on_click: None,
            on_right_click: None,
            children: Vec::new(),
        }
    }

    /// 使容器占用弹性布局的剩余空间。
    pub fn flex_grow(mut self) -> Self {
        self.flex_grow = true;
        self
    }

    pub fn padding(mut self, padding: impl Into<DefiniteLength>) -> Self {
        self.padding = padding.into();
        self
    }

    /// 设置悬停提示。
    pub fn tooltip(mut self, tooltip: TooltipSpec) -> Self {
        self.tooltip = tooltip;
        self
    }

    pub(crate) fn style(mut self, style: ButtonStyle) -> Self {
        self.style = style;
        self
    }

    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub(crate) fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// 遮蔽下层鼠标交互，保留滚轮事件。
    pub(crate) fn occlude(mut self, occlude: bool) -> Self {
        self.occlude = occlude;
        self
    }

    pub(crate) fn shortcut(mut self, shortcut: Option<impl Into<String>>) -> Self {
        self.tooltip = self.tooltip.with_shortcut(shortcut);
        self
    }

    /// 设置左键点击回调。
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// 设置右键点击回调。
    pub fn on_right_click(
        mut self,
        handler: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_right_click = Some(Rc::new(handler));
        self
    }
}

impl ParentElement for ButtonLike {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl IntoElement for ButtonLike {
    type Element = ViewElement<Self>;

    fn into_element(self) -> Self::Element {
        ViewElement::new(self)
    }
}

impl ButtonLike {
    pub(crate) fn render_root(self, cx: &App) -> Stateful<Div> {
        let interactive = self.on_click.is_some() || self.on_right_click.is_some();
        let colors = *color::current(cx);
        let mut element = div()
            .id(self.id)
            .flex_none()
            .rounded_sm()
            .p(self.padding)
            .when(self.occlude, |element| element.block_mouse_except_scroll())
            .when(self.flex_grow, |element| element.flex_1().min_w_0());

        if self.style == ButtonStyle::Solid {
            element = element
                .border_1()
                .border_color(colors.border)
                .bg(colors.panel_background);
        }

        if let Some(cursor) = cursor_for_state(self.disabled, interactive) {
            element = element.cursor(cursor);
        }
        if !self.disabled {
            let hover = match self.style {
                ButtonStyle::Ghost => colors.ghost_element_hover,
                ButtonStyle::Solid => colors.element_hover,
            };
            element = element.hover(move |style| style.bg(hover));
            if interactive {
                element = element.active(move |style| style.bg(hover));
            }
        }
        if !self.disabled
            && let Some(on_click) = self.on_click
        {
            element = element
                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                    cx.stop_propagation();
                })
                .on_click(move |event, window, cx| {
                    on_click(event, window, cx);
                    cx.stop_propagation();
                });
        }
        if !self.disabled
            && let Some(on_right_click) = self.on_right_click
        {
            element = element
                .on_mouse_down(MouseButton::Right, |_event, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_mouse_up(MouseButton::Right, move |event, window, cx| {
                    on_right_click(event, window, cx);
                    cx.stop_propagation();
                });
        }
        if let Some(build) = self.tooltip.build() {
            element = element.tooltip(build);
        }

        element.children(self.children)
    }
}

impl RenderOnce for ButtonLike {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        self.render_root(cx)
    }
}

#[cfg(test)]
#[path = "test/button_like_tests.rs"]
mod tests;
