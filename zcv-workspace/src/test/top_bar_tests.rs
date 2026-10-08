use std::cell::RefCell;

use gpui::{Bounds, Context, Pixels, TestAppContext, canvas, px};

use super::*;

struct TopBarLayout {
    frame_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    search_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
}

impl gpui::Render for TopBarLayout {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let frame_bounds = self.frame_bounds.clone();
        let search_bounds = self.search_bounds.clone();
        bar_frame(cx)
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
