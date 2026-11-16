use std::{cell::Cell, rc::Rc};

use gpui::{Context, KeyUpEvent, Keystroke, Modifiers, Render, TestAppContext};

use super::*;

struct SwitchExample {
    checked: bool,
    requested_value: Rc<Cell<Option<bool>>>,
}

impl Render for SwitchExample {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let requested_value = self.requested_value.clone();
        let window = cx.entity().downgrade();
        Switch::new("enable", self.checked, move |value, _, cx| {
            requested_value.set(Some(value));
            window
                .update(cx, |this, cx| {
                    this.checked = value;
                    cx.notify();
                })
                .ok();
        })
    }
}

#[gpui::test]
fn switch_click_and_keyboard_request_the_inverse_state(cx: &mut TestAppContext) {
    let requested_value = Rc::new(Cell::new(None));
    let (_, visual) = cx.add_window_view(|_, _| SwitchExample {
        checked: false,
        requested_value: requested_value.clone(),
    });
    let track = visual.debug_bounds("switch-track").expect("开关应可见");
    visual.simulate_click(track.center(), Modifiers::default());
    assert_eq!(requested_value.get(), Some(true), "点击应请求开启");

    assert!(
        visual.debug_bounds("switch-track").is_some(),
        "切换后仍应显示开关"
    );
    visual.update(|window, cx| {
        window.blur(cx);
        window.focus_next(cx);
    });
    visual.simulate_keystrokes("enter");
    visual.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("enter").expect("Enter 应能解析"),
    });
    assert_eq!(requested_value.get(), Some(false), "Enter 应请求关闭");

    visual.simulate_keystrokes("space");
    visual.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("space").expect("空格键应能解析"),
    });
    assert_eq!(requested_value.get(), Some(true), "空格键应请求开启");
}
