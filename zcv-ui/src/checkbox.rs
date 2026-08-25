//! Checkbox —— 复选方框组件。
//!
//! 组件自封装视觉与悬停提示，点击回调由调用方注入。

use std::rc::Rc;

use gpui::{
    Action, App, Component, ElementId, IntoElement, MouseButton, RenderOnce, Window, div,
    prelude::*,
};
use zcv_theme::{color, typography};

use crate::{SvgIcon, TooltipSpec};

type ClickHandler = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct Checkbox {
    id: ElementId,
    checked: bool,
    tooltip: TooltipSpec,
    on_click: Option<ClickHandler>,
}

impl Checkbox {
    /// 构造复选框。`id` 需在所在作用域内唯一；`checked` 决定勾选状态。
    pub fn new(id: impl Into<ElementId>, checked: bool) -> Self {
        Self {
            id: id.into(),
            checked,
            tooltip: TooltipSpec::default(),
            on_click: None,
        }
    }

    /// 设置悬停提示文字。
    pub fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip = TooltipSpec::new(text);
        self
    }

    /// 关联 action：快捷键从 keymap 查询并显示在悬停提示里。
    pub fn shortcut(mut self, action: &dyn Action, cx: &App) -> Self {
        self.tooltip = self.tooltip.with_action(action, cx);
        self
    }

    /// 设置单击回调。
    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl IntoElement for Checkbox {
    type Element = Component<Self>;

    fn into_element(self) -> Self::Element {
        Component::new(self)
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = color::current(cx);
        let tooltip = self.tooltip;
        let on_click = self.on_click;
        let hover_border = {
            let mut border = colors.border;
            border.a = (border.a * 0.7).clamp(0.0, 1.0);
            border
        };
        div()
            .id(self.id)
            .size(typography::ui())
            .rounded_xs()
            .border_1()
            .bg(colors.ghost_element_background)
            .border_color(colors.border)
            .hover(|style| style.border_color(hover_border))
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .cursor_pointer()
            .when_some(tooltip.build(), |el, build| el.tooltip(build))
            .when(self.checked, |el| {
                el.child(
                    SvgIcon::new("icons/check.svg")
                        .size(typography::ui())
                        .color(colors.icon_accent)
                        .into_any_element(),
                )
            })
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                if event.click_count == 1
                    && let Some(handler) = &on_click
                {
                    handler(window, cx);
                }
                // 复选框是行内交互：阻止冒泡到行的选中/打开逻辑。
                cx.stop_propagation();
            })
    }
}
