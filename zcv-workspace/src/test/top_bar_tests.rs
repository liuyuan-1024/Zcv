use std::cell::RefCell;

use gpui::{Bounds, Context, Pixels, TestAppContext, canvas, px};

use super::*;

struct TopBarLayout {
    frame_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    search_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
}

impl gpui::Render for TopBarLayout {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let frame_bounds = self.frame_bounds.clone();
        let search_bounds = self.search_bounds.clone();
        bar_frame(bar_height(window, cx), cx)
            .child(
                canvas(
                    move |bounds, _, _| *frame_bounds.borrow_mut() = Some(bounds),
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(div().w(px(12.0)).h(px(40.0)))
            .child(
                canvas(
                    move |bounds, _, _| *search_bounds.borrow_mut() = Some(bounds),
                    |_, _, _, _| {},
                )
                .w(px(120.0))
                .h(px(20.0)),
            )
            .child(bar_bottom_border(cx))
    }
}

#[gpui::test]
fn top_bar_centers_search_with_a_bottom_border(cx: &mut TestAppContext) {
    let frame_bounds = Rc::new(RefCell::new(None));
    let search_bounds = Rc::new(RefCell::new(None));
    let _window = cx.add_window(|_, _| TopBarLayout {
        frame_bounds: frame_bounds.clone(),
        search_bounds: search_bounds.clone(),
    });
    cx.run_until_parked();

    let frame = frame_bounds.borrow().expect("顶栏应完成布局");
    let search = search_bounds.borrow().expect("搜索框应完成布局");
    let frame_center = f32::from(frame.top()) + f32::from(frame.size.height) / 2.0;
    let search_center = f32::from(search.top()) + f32::from(search.size.height) / 2.0;
    assert_eq!(frame_center, search_center);
}

#[cfg(target_os = "macos")]
struct NativeControlsInsetLayout {
    content_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
}

#[cfg(target_os = "macos")]
impl gpui::Render for NativeControlsInsetLayout {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let content_bounds = self.content_bounds.clone();
        bar_frame(bar_height(window, cx), cx).child(content_frame(window).child(canvas(
            move |bounds, _, _| *content_bounds.borrow_mut() = Some(bounds),
            |_, _, _, _| {},
        )))
    }
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn top_bar_content_starts_after_native_window_controls(cx: &mut TestAppContext) {
    let content_bounds = Rc::new(RefCell::new(None));
    let (_, cx) = cx.add_window_view(|window, _| {
        window.set_rem_size(px(scale::DEFAULT_UI_SIZE));
        NativeControlsInsetLayout {
            content_bounds: content_bounds.clone(),
        }
    });
    cx.run_until_parked();

    let content = content_bounds.borrow().expect("顶栏内容应完成布局");
    assert_eq!(content.left(), px(78.0));

    cx.update(|window, _| {
        window.set_rem_size(px(scale::DEFAULT_UI_SIZE * 2.0));
        window.refresh();
    });
    cx.run_until_parked();

    let content = content_bounds
        .borrow()
        .expect("调整字号后顶栏内容应重新布局");
    assert_eq!(content.left(), px(96.0));
}
