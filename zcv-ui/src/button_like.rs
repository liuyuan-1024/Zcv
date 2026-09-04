//! `ButtonLike` —— 为自定义内容提供统一鼠标交互的轻量容器。
//!
//! 适用于内容布局不属于标准图文按钮、但仍需要悬停提示或鼠标操作的场景。

use std::rc::Rc;

use gpui::{
    App, ClickEvent, Component, ElementId, IntoElement, MouseButton, MouseUpEvent, ParentElement,
    RenderOnce, Window, div, prelude::*,
};
use zcv_theme::{color, space};

use crate::TooltipSpec;

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type RightClickHandler = Rc<dyn Fn(&MouseUpEvent, &mut Window, &mut App)>;

/// 自定义内容的鼠标交互容器。
pub struct ButtonLike {
    id: ElementId,
    flex_grow: bool,
    tooltip: TooltipSpec,
    on_click: Option<ClickHandler>,
    on_right_click: Option<RightClickHandler>,
    children: Vec<gpui::AnyElement>,
}

impl ButtonLike {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            flex_grow: false,
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

    /// 设置悬停提示。
    pub fn tooltip(mut self, tooltip: TooltipSpec) -> Self {
        self.tooltip = tooltip;
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
    fn extend(&mut self, elements: impl IntoIterator<Item = gpui::AnyElement>) {
        self.children.extend(elements);
    }
}

impl IntoElement for ButtonLike {
    type Element = Component<Self>;

    fn into_element(self) -> Self::Element {
        Component::new(self)
    }
}

impl RenderOnce for ButtonLike {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let interactive = self.on_click.is_some() || self.on_right_click.is_some();
        let colors = *color::current(cx);
        let mut element = div()
            .id(self.id)
            .flex_none()
            .rounded_sm()
            .p(space::S2)
            .when(self.flex_grow, |element| element.flex_1().min_w_0());

        if interactive {
            element = element
                .cursor_pointer()
                .hover(move |style| style.bg(colors.ghost_element_hover))
                .active(move |style| style.bg(colors.ghost_element_hover));
        }
        if let Some(on_click) = self.on_click {
            element = element
                .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                    cx.stop_propagation();
                })
                .on_click(move |event, window, cx| {
                    on_click(event, window, cx);
                    cx.stop_propagation();
                });
        }
        if let Some(on_right_click) = self.on_right_click {
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
