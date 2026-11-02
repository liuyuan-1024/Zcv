//! 浮层呈现原语的行为测试：建议宽度与窗口收缩。

use gpui::{
    Pixels, Render, Size, TestAppContext, VisualTestContext, Window, div, prelude::*, px, size,
};

use crate::{OverlayAnchor, overlay};

/// 把浮层挂进窗口根节点，使浮层面板的实际布局可被测量。
struct OverlayHost;

impl Render for OverlayHost {
    fn render(&mut self, window: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        overlay(window)
            .anchor(OverlayAnchor::AtSlot)
            .min_width(px(200.0))
            .child(div())
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
