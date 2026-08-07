//! Checkbox —— 复选方框组件。
//!
//! 对齐 Zed `ui::Checkbox`（crates/ui/src/components/toggle.rs）的形态与用法：
//! 组件自封装视觉与悬停提示，点击回调由调用方注入。
//! 视觉：16px 圆角方框 + 边框，勾选时框内显示高亮色对勾。

use std::rc::Rc;

use gpui::{
    App, Component, ElementId, IntoElement, MouseButton, RenderOnce, Window, div, prelude::*, px,
};

use crate::ui::{SvgIcon, tooltip_for_action, tooltip_view};
use zcv_theme::{color, typography};

type ClickHandler = Rc<dyn Fn(&mut Window, &mut App)>;

pub(crate) struct Checkbox {
    id: ElementId,
    checked: bool,
    tooltip_text: Option<String>,
    /// 关联的 action 名称：快捷键的查询与显示由 Tooltip 组件负责。
    tooltip_action: Option<&'static str>,
    on_click: Option<ClickHandler>,
}

impl Checkbox {
    /// 构造复选框。`id` 需在所在作用域内唯一；`checked` 决定勾选状态。
    pub(crate) fn new(id: impl Into<ElementId>, checked: bool) -> Self {
        Self {
            id: id.into(),
            checked,
            tooltip_text: None,
            tooltip_action: None,
            on_click: None,
        }
    }

    /// 设置悬停提示文字。
    pub(crate) fn tooltip(mut self, text: impl Into<String>) -> Self {
        self.tooltip_text = Some(text.into());
        self
    }

    /// 关联 action：悬停提示里由 Tooltip 查询并显示对应快捷键。
    pub(crate) fn shortcut(mut self, action_name: &'static str) -> Self {
        self.tooltip_action = Some(action_name);
        self
    }

    /// 设置单击回调。
    pub(crate) fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
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
        let tooltip_text = self.tooltip_text;
        let tooltip_action = self.tooltip_action;
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
            .when_some(tooltip_text, |el, text| {
                el.tooltip(move |_, cx| match tooltip_action {
                    Some(action_name) => tooltip_for_action(text.clone(), action_name, cx),
                    None => tooltip_view(cx, Some(text.clone()), None),
                })
            })
            .when(self.checked, |el| {
                el.child(
                    SvgIcon::new("icons/actions/check.svg")
                        .size(typography::ui())
                        .color(colors.icon_accent)
                        .into_any_element(),
                )
            })
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                if event.click_count == 1 {
                    if let Some(handler) = &on_click {
                        handler(window, cx);
                    }
                }
                // 复选框是行内交互：阻止冒泡到行的选中/打开逻辑。
                cx.stop_propagation();
            })
    }
}
