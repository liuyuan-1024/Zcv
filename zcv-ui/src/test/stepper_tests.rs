use std::{cell::Cell, rc::Rc};

use gpui::{Context, KeyUpEvent, Keystroke, Modifiers, Render, TestAppContext, px};

use super::*;

struct StepperExample {
    increased: Rc<Cell<usize>>,
}

impl Render for StepperExample {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let increased = self.increased.clone();
        div().flex().items_start().child(
            Stepper::new("indent", "缩进宽度", "1 列")
                .on_increase(move |_, _| increased.set(increased.get() + 1)),
        )
    }
}

#[gpui::test]
fn stepper_only_activates_enabled_directions(cx: &mut TestAppContext) {
    let increased = Rc::new(Cell::new(0));
    let (_, visual) = cx.add_window_view(|_, _| StepperExample {
        increased: increased.clone(),
    });
    let stepper = visual.debug_bounds("stepper").expect("步进器应可见");
    let decrease = visual
        .debug_bounds("stepper-decrease")
        .expect("减小按钮应可见");
    let increase = visual
        .debug_bounds("stepper-increase")
        .expect("增大按钮应可见");
    let top_inset = decrease.top() - stepper.top();
    assert!(top_inset > px(1.0), "按钮与边框之间应有内边距");
    for inset in [
        decrease.left() - stepper.left(),
        stepper.right() - increase.right(),
        stepper.bottom() - decrease.bottom(),
    ] {
        assert!(
            f32::from(inset - top_inset).abs() <= 1.0,
            "步进器四边内边距应一致：{inset:?} 对 {top_inset:?}",
        );
    }
    visual.simulate_click(decrease.center(), Modifiers::default());
    assert_eq!(increased.get(), 0, "禁用的减小按钮不应触发操作");

    visual.update(|window, cx| window.focus_next(cx));
    visual.simulate_keystrokes("enter");
    visual.simulate_event(KeyUpEvent {
        keystroke: Keystroke::parse("enter").expect("Enter 应能解析"),
    });
    assert_eq!(increased.get(), 1, "键盘焦点应跳过禁用的减小按钮");

    let increase = visual
        .debug_bounds("stepper-increase")
        .expect("增大按钮应可见");
    visual.simulate_click(increase.center(), Modifiers::default());
    assert_eq!(increased.get(), 2, "点击应触发增大操作");
}
