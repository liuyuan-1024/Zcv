//! 树行的折叠控件；展开状态由所在面板提供。

use gpui::{
    App, ElementId, IntoElement, MouseButton, RenderOnce, Role, ViewElement, Window, div,
    prelude::*,
};
use zcv_theme::color;

use crate::{SvgIcon, TooltipSpec};

pub struct TreeDisclosure {
    id: ElementId,
    expanded: bool,
    on_toggle: Box<dyn Fn(&mut App)>,
}

impl TreeDisclosure {
    pub fn new(
        id: impl Into<ElementId>,
        expanded: bool,
        on_toggle: impl Fn(&mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            expanded,
            on_toggle: Box::new(on_toggle),
        }
    }
}

impl IntoElement for TreeDisclosure {
    type Element = ViewElement<Self>;

    fn into_element(self) -> Self::Element {
        ViewElement::new(self)
    }
}

impl RenderOnce for TreeDisclosure {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let label = if self.expanded { "折叠" } else { "展开" };
        let icon = if self.expanded {
            "icons/chevron_down.svg"
        } else {
            "icons/chevron_right.svg"
        };
        let colors = *color::current(cx);
        let on_toggle = self.on_toggle;

        div()
            .id(self.id)
            .role(Role::Button)
            .aria_label(label)
            .aria_expanded(self.expanded)
            .size(window.rem_size())
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .cursor_pointer()
            .hover(move |style| style.bg(colors.ghost_element_hover))
            .when_some(TooltipSpec::from_lines([label]).build(), |el, build| {
                el.tooltip(build)
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |event, _, cx| {
                if event.is_right_click() {
                    return;
                }
                on_toggle(cx);
                cx.stop_propagation();
            })
            .child(
                SvgIcon::new(icon)
                    .size(window.rem_size())
                    .color(colors.icon_muted),
            )
    }
}
