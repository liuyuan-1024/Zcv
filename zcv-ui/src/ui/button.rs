//! `Button` —— 通用纯文本操作按钮。
//!
//! 与 `Glyph` 分工：Button 承载文字命令，Glyph 承载图标命令；
//! 两者都封装稳定 id、主题状态和点击交互，调用方不再重复手写按钮外观。

use std::rc::Rc;

use gpui::{
    App, ClickEvent, Component, ElementId, IntoElement, RenderOnce, Window, div, prelude::*,
};
use zcv_theme::{color, space};

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

pub struct Button {
    id: ElementId,
    label: String,
    text_color: Option<gpui::Rgba>,
    on_click: Option<ClickHandler>,
    disabled: bool,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            text_color: None,
            on_click: None,
            disabled: false,
        }
    }

    pub fn text_color(mut self, color: gpui::Rgba) -> Self {
        self.text_color = Some(color);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
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
        let mut element = div()
            .id(self.id)
            .px(space::S12)
            .py(space::S6)
            .rounded_md()
            .border_1()
            .border_color(colors.border_variant)
            .bg(colors.panel_background)
            .text_color(self.text_color.unwrap_or(if disabled {
                colors.text_disabled
            } else {
                colors.text
            }))
            .child(self.label);

        if !disabled {
            element = element
                .cursor_pointer()
                .hover(move |style| style.bg(colors.element_hover));
            if let Some(handler) = on_click {
                element = element.on_click(move |event, window, cx| handler(event, window, cx));
            }
        }

        element
    }
}
