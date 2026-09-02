//! Harness —— 底栏右侧的模式状态标记按钮。
//!
//! 当前仅维护开关状态并高亮显示，具体功能后续接入。

use gpui::{Context, ElementId, Render, Window, div, prelude::*};
use zcv_actions::ToggleHarnessMode;
use zcv_theme::{color, space};
use zcv_ui::Button;
use zcv_workspace::{ItemHandle, StatusItemView};

pub(crate) struct HarnessButton {
    harness_on: bool,
}

impl HarnessButton {
    pub(crate) fn new() -> Self {
        Self { harness_on: false }
    }

    /// 切换模式标记；键盘与鼠标入口共用同一实现。
    pub(crate) fn toggle(&mut self, cx: &mut Context<Self>) {
        self.harness_on = !self.harness_on;
        cx.notify();
    }
}

impl StatusItemView for HarnessButton {
    fn set_active_pane_item(&mut self, _item: Option<&dyn ItemHandle>, _cx: &mut Context<Self>) {
        // 不追踪编辑器状态。
    }
}

impl Render for HarnessButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let fg = if self.harness_on {
            color::current(cx).icon_accent
        } else {
            color::current(cx).text
        };
        // 与右侧面板按钮组同款的前导分隔线，承担与底栏其他状态项的视觉分隔。
        let divider = div().w(space::S1).h_full().bg(color::current(cx).border);
        div()
            .flex()
            .items_center()
            .gap(space::S6)
            .child(divider)
            .child(
                Button::icon(ElementId::Name("harness".into()), "icons/zed_assistant.svg")
                    .label("Harness 模式")
                    .shortcut(&ToggleHarnessMode, cx)
                    .color(fg)
                    .on_click(cx.listener(|button, _, _window, cx| button.toggle(cx))),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext, TestAppContext};

    use super::*;

    #[gpui::test]
    fn toggle_flips_marker_state(cx: &mut TestAppContext) {
        let button = cx.new(|_| HarnessButton::new());
        cx.read_entity(&button, |button, _| assert!(!button.harness_on));
        cx.update_entity(&button, |button, cx| button.toggle(cx));
        cx.read_entity(&button, |button, _| assert!(button.harness_on));
        cx.update_entity(&button, |button, cx| button.toggle(cx));
        cx.read_entity(&button, |button, _| assert!(!button.harness_on));
    }
}
