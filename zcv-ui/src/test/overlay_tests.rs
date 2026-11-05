//! 浮层呈现原语的行为测试：建议宽度与窗口收缩。

use std::{cell::Cell, rc::Rc};

use gpui::{
    Modifiers, MouseButton, Pixels, Render, Size, TestAppContext, VisualTestContext, Window, div,
    point, prelude::*, px, size,
};

use crate::{OverlayOutside, overlay};

/// 把浮层挂进窗口根节点，使浮层面板的实际布局可被测量。
struct OverlayHost;

impl Render for OverlayHost {
    fn render(&mut self, window: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        overlay(window).min_width(px(200.0)).child(div())
    }
}

/// 挂载指定窗口尺寸的浮层，返回窗口级上下文。
fn open_overlay(cx: &mut TestAppContext, window_size: Size<Pixels>) -> &mut VisualTestContext {
    let (_, visual) = cx.add_window_view(|_, _| OverlayHost);
    let window = *visual.windows().last().expect("窗口应已创建");
    visual.simulate_window_resize(window, window_size);
    visual
}

/// 短内容浮层保持建议宽度，不被内容宽度限制得局促。
#[gpui::test]
fn suggested_width_floors_short_content(cx: &mut TestAppContext) {
    let visual = open_overlay(cx, size(px(720.0), px(600.0)));
    let width = visual
        .debug_bounds("overlay-panel")
        .expect("浮层面板应参与布局")
        .size
        .width;
    assert!(
        width >= px(200.0),
        "短内容浮层应保持建议宽度：实际 {width:?}"
    );
}

/// 窗口不够宽时浮层收缩到窗口内，不超出窗口边界。
#[gpui::test]
fn width_shrinks_in_narrow_window(cx: &mut TestAppContext) {
    let visual = open_overlay(cx, size(px(150.0), px(300.0)));
    let width = visual
        .debug_bounds("overlay-panel")
        .expect("窄窗口浮层应参与布局")
        .size
        .width;
    assert!(
        width <= px(150.0),
        "窄窗口中浮层不应超出窗口宽度：实际 {width:?}"
    );
    assert!(
        width >= px(130.0),
        "窄窗口中浮层应尽量利用窗口宽度：实际 {width:?}"
    );
}

struct OutsideClickHost {
    dismissed: Rc<Cell<usize>>,
    background_clicks: Rc<Cell<usize>>,
}

impl Render for OutsideClickHost {
    fn render(&mut self, window: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        let dismissed = self.dismissed.clone();
        let background_clicks = self.background_clicks.clone();
        div()
            .size_full()
            .on_mouse_down(MouseButton::Left, move |_, _, _| {
                background_clicks.set(background_clicks.get() + 1);
            })
            .child(
                div().ml(px(220.0)).mt(px(180.0)).child(
                    overlay(window)
                        .outside(OverlayOutside::Intercept(Box::new(move |_, _, _| {
                            dismissed.set(dismissed.get() + 1);
                        })))
                        .child(div().w(px(120.0)).h(px(80.0))),
                ),
            )
    }
}

#[gpui::test]
fn intercept_covers_window_outside_local_trigger(cx: &mut TestAppContext) {
    let dismissed = Rc::new(Cell::new(0));
    let background_clicks = Rc::new(Cell::new(0));
    let (_, visual) = cx.add_window_view(|_, _| OutsideClickHost {
        dismissed: dismissed.clone(),
        background_clicks: background_clicks.clone(),
    });
    let window = *visual.windows().last().expect("窗口应已创建");
    visual.simulate_window_resize(window, size(px(720.0), px(600.0)));

    let panel = visual
        .debug_bounds("overlay-panel")
        .expect("浮层面板应参与布局");
    visual.simulate_click(panel.center(), Modifiers::default());
    assert_eq!(dismissed.get(), 0, "面板内部点击应保持浮层打开");

    visual.simulate_click(point(px(20.0), px(20.0)), Modifiers::default());
    assert_eq!(dismissed.get(), 1, "触发器左上方空白应关闭浮层");
    visual.simulate_click(point(px(700.0), px(580.0)), Modifiers::default());
    assert_eq!(dismissed.get(), 2, "窗口两侧空白都应关闭浮层");
    assert_eq!(background_clicks.get(), 0, "拦截式浮层不应激活下层");
}
