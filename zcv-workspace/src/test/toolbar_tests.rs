use gpui::{FocusHandle, Focusable, TestAppContext, div, prelude::*};

use super::*;
use crate::Item;

struct TestItem {
    focus: FocusHandle,
}

impl EventEmitter<()> for TestItem {}

impl Focusable for TestItem {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TestItem {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Item for TestItem {
    type Event = ();

    fn tab_content_text(&self, _cx: &App) -> gpui::SharedString {
        "测试 Item".into()
    }
}

struct TestView;

impl Render for TestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// 活动 Item 为 None 时落在 PrimaryLeft，出现活动 Item 后移到 PrimaryRight。
struct LocationProbe;

impl EventEmitter<ToolbarItemEvent> for LocationProbe {}

impl Render for LocationProbe {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl ToolbarItemView for LocationProbe {
    fn set_active_pane_item(
        &mut self,
        item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        if item.is_some() {
            ToolbarItemLocation::PrimaryRight
        } else {
            ToolbarItemLocation::PrimaryLeft
        }
    }
}

#[gpui::test]
fn active_item_change_repositions_toolbar_items(cx: &mut TestAppContext) {
    let toolbar = cx.new(|_| Toolbar::new());
    let active = cx.new(|cx| TestItem {
        focus: cx.focus_handle(),
    });
    cx.add_window_view(|window, cx| {
        let probe = cx.new(|_| LocationProbe);
        toolbar.update(cx, |toolbar, cx| toolbar.add_item(probe, window, cx));
        assert_eq!(
            toolbar.read(cx).items[0].1,
            ToolbarItemLocation::PrimaryLeft,
            "无活动 Item 时应落在初始位置"
        );

        toolbar.update(cx, |toolbar, cx| {
            toolbar.set_active_item(Some(&active as &dyn ItemHandle), window, cx)
        });
        assert_eq!(
            toolbar.read(cx).items[0].1,
            ToolbarItemLocation::PrimaryRight,
            "活动 Item 变化后工具项应按新位置重排"
        );
        TestView
    });
}
