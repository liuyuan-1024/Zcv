use super::*;
use std::{cell::Cell, rc::Rc};

use gpui::{Context, Render, TestAppContext, Window, div, prelude::*};

#[derive(Clone)]
struct TestScrollHandle {
    requested_offset: Rc<Cell<Pixels>>,
    drag_started: Rc<Cell<bool>>,
}

impl ScrollableHandle for TestScrollHandle {
    fn max_offset(&self) -> Point<Pixels> {
        point(Pixels::ZERO, px(1000.))
    }

    fn set_offset(&self, point: Point<Pixels>) {
        self.requested_offset.set(point.y);
    }

    fn offset(&self) -> Point<Pixels> {
        // 模拟异步容器：拖拽事件之间，已呈现的偏移还没有更新。
        point(Pixels::ZERO, Pixels::ZERO)
    }

    fn viewport(&self) -> Bounds<Pixels> {
        Bounds::default()
    }

    fn drag_started(&self) {
        self.drag_started.set(true);
    }
}

struct ScrollbarTestView {
    scrollbar: Scrollbar<TestScrollHandle>,
}

impl Render for ScrollbarTestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .relative()
            .child(div().absolute().inset_0().child(self.scrollbar.clone()))
    }
}

#[gpui::test]
fn dragging_the_scrollbar_thumb_updates_its_handle(cx: &mut TestAppContext) {
    let handle = TestScrollHandle {
        requested_offset: Rc::new(Cell::new(Pixels::ZERO)),
        drag_started: Rc::new(Cell::new(false)),
    };
    let scrollbar = Scrollbar::vertical(handle.clone());
    let (_, cx) = cx.add_window_view(move |_window, _cx| ScrollbarTestView { scrollbar });
    cx.refresh().expect("测试窗口应完成首次绘制");

    let (right, top) = cx.update(|window, _| (window.bounds().right(), window.bounds().top()));
    let thumb = point(right - px(4.), top + px(10.));
    cx.simulate_mouse_down(thumb, MouseButton::Left, gpui::Modifiers::default());
    assert!(handle.drag_started.get(), "按下滑块应开始拖拽");
    cx.refresh().expect("按下滑块后应刷新");
    cx.simulate_mouse_move(
        point(thumb.x, thumb.y + px(120.)),
        Some(MouseButton::Left),
        gpui::Modifiers::default(),
    );
    let first_offset = handle.requested_offset.get();
    cx.simulate_mouse_move(
        point(thumb.x, thumb.y + px(240.)),
        Some(MouseButton::Left),
        gpui::Modifiers::default(),
    );

    assert!(
        handle.requested_offset.get() < first_offset,
        "异步偏移尚未刷新时，连续拖动仍应累积"
    );
}
