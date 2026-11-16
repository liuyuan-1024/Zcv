use gpui::{App, ElementId, SharedString, Window, div, prelude::*};
use zcv_theme::{color, scale, typography};

use crate::Button;

type StepAction = Box<dyn Fn(&mut Window, &mut App)>;

/// 将数值和加减按钮组合为一个控件，支持鼠标和键盘操作。
///
/// 调用方持有数值，负责单位、步长、边界和实际更新。每次激活按钮只请求一次调整。
/// 没有配置回调的方向显示为禁用，并从键盘焦点顺序中跳过。
#[derive(IntoElement)]
pub struct Stepper {
    id: ElementId,
    label: SharedString,
    value: SharedString,
    decrease: Option<StepAction>,
    increase: Option<StepAction>,
}

impl Stepper {
    /// 创建数值步进器。为每个步进器提供在所在作用域内唯一的 `id`。
    ///
    /// `label` 用于生成加减按钮的悬停提示。
    /// `value` 是显示文本，需要时包含单位，例如 `"1000 ms"`。
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        value: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            value: value.into(),
            decrease: None,
            increase: None,
        }
    }

    /// 启用减小按钮，并在用户激活它时请求减小当前值。
    pub fn on_decrease(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.decrease = Some(Box::new(action));
        self
    }

    /// 启用增大按钮，并在用户激活它时请求增大当前值。
    pub fn on_increase(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.increase = Some(Box::new(action));
        self
    }
}

impl RenderOnce for Stepper {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = *color::current(cx);
        let ui_size = window.rem_size();
        div()
            .id(self.id)
            .debug_selector(|| "stepper".into())
            .flex()
            .flex_none()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(colors.border)
            .bg(colors.panel_background)
            .p(scale::S2)
            .child(step_button(
                "decrease",
                "icons/dash.svg",
                format!("减小{}", self.label),
                self.decrease,
            ))
            .child(
                div()
                    .min_w(scale::S32)
                    .px(scale::S2)
                    .text_center()
                    .text_size(ui_size)
                    .line_height(typography::ui_line_at(ui_size, cx))
                    .text_color(colors.text)
                    .whitespace_nowrap()
                    .child(self.value),
            )
            .child(step_button(
                "increase",
                "icons/plus.svg",
                format!("增大{}", self.label),
                self.increase,
            ))
    }
}

fn step_button(
    id: &'static str,
    icon: &'static str,
    label: String,
    action: Option<StepAction>,
) -> impl IntoElement {
    let button = Button::icon(id, icon).label(label).keyboard_focusable();
    let button = match action {
        Some(action) => button.on_click(move |_, window, cx| action(window, cx)),
        None => button.disabled(true),
    };
    div()
        .debug_selector(move || format!("stepper-{id}"))
        .child(button)
}

#[cfg(test)]
#[path = "test/stepper_tests.rs"]
mod tests;
