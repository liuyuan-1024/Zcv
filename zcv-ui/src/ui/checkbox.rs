//! Checkbox —— 复选方框组件。
//!
//! 对齐 Zed `ui::Checkbox`（crates/ui/src/components/toggle.rs）的形态与用法：
//! 组件自封装视觉与悬停提示，点击回调由调用方注入。
//! 视觉：16px 圆角方框 + 边框，勾选时框内显示高亮色对勾。

use std::rc::Rc;

use gpui::{
    Action, App, Component, ElementId, IntoElement, MouseButton, RenderOnce, Window, div,
    prelude::*, px,
};

use crate::ui::{SvgIcon, TooltipSpec};
use zcv_keymap::KeyBindings;
use zcv_theme::{color, typography};

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
        if let Some(s) = cx
            .try_global::<KeyBindings>()
            .and_then(|kb| kb.display_shortcut(action.name()))
        {
            self.tooltip = self.tooltip.shortcut(s);
        }
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
        div()
            .id(self.id)
            .size(px(16.0))
            .rounded_xs()
            .border_1()
            .border_color(colors.border_variant)
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
