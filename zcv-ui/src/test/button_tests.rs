use gpui::{Context, Render, TestAppContext};
use gpui::{ScrollDelta, ScrollHandle, ScrollWheelEvent, point, px};
use std::cell::Cell;
use std::rc::Rc;

use super::*;

#[test]
fn disabled_state_uses_not_allowed_cursor() {
    assert_eq!(
        cursor_for_state(true, true),
        Some(CursorStyle::OperationNotAllowed)
    );
    assert_eq!(
        cursor_for_state(true, false),
        Some(CursorStyle::OperationNotAllowed)
    );
}

#[test]
fn enabled_state_preserves_existing_interaction() {
    assert_eq!(
        cursor_for_state(false, true),
        Some(CursorStyle::PointingHand)
    );
    assert_eq!(cursor_for_state(false, false), None);
}

fn expected_height(ui_line: Pixels, size: ButtonSize) -> Pixels {
    ui_line + size.padding() * 2.0
}

struct ButtonHeightHost {
    compact_height: Pixels,
}

impl Render for ButtonHeightHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .debug_selector(|| "icon-button".into())
                    .child(Button::icon("icon", "icons/close.svg")),
            )
            .child(
                div()
                    .debug_selector(|| "text-button".into())
                    .child(Button::text("text", "关闭").style(ButtonStyle::Solid)),
            )
            .child(
                div()
                    .debug_selector(|| "icon-text-button".into())
                    .child(Button::icon_text("icon-text", "icons/close.svg", "关闭")),
            )
    }
}

#[gpui::test]
fn content_and_visual_style_share_the_default_height(cx: &mut TestAppContext) {
    let (host, cx) = cx.add_window_view(|window, cx| ButtonHeightHost {
        compact_height: expected_height(
            typography::ui_line_at(window.rem_size(), cx),
            ButtonSize::Compact,
        ),
    });
    let icon = cx.debug_bounds("icon-button").expect("图标按钮应参与布局");
    let text = cx.debug_bounds("text-button").expect("文字按钮应参与布局");
    let icon_text = cx
        .debug_bounds("icon-text-button")
        .expect("图文按钮应参与布局");
    let compact_height = cx.read_entity(&host, |host, _| host.compact_height);

    assert_eq!(icon.size.height, text.size.height);
    assert_eq!(text.size.height, icon_text.size.height);
    assert_eq!(icon.size.height, compact_height);
}

struct LooseButtonHost {
    compact_height: Pixels,
    loose_height: Pixels,
}

impl Render for LooseButtonHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .child(
                div().debug_selector(|| "loose-icon".into()).child(
                    Button::icon("loose-icon-btn", "icons/close.svg").size(ButtonSize::Loose),
                ),
            )
            .child(
                div().debug_selector(|| "loose-text".into()).child(
                    Button::text("loose-text-btn", "确定")
                        .size(ButtonSize::Loose)
                        .style(ButtonStyle::Solid),
                ),
            )
    }
}

#[gpui::test]
fn loose_size_scales_height_and_padding(cx: &mut TestAppContext) {
    let (host, cx) = cx.add_window_view(|window, cx| LooseButtonHost {
        compact_height: expected_height(
            typography::ui_line_at(window.rem_size(), cx),
            ButtonSize::Compact,
        ),
        loose_height: expected_height(
            typography::ui_line_at(window.rem_size(), cx),
            ButtonSize::Loose,
        ),
    });
    let icon = cx
        .debug_bounds("loose-icon")
        .expect("宽松图标按钮应参与布局");
    let text = cx
        .debug_bounds("loose-text")
        .expect("宽松文字按钮应参与布局");

    // 宽松高度 = 字号 + S6×2，与紧凑档位差 2×(S6−S2)。
    let (compact_height, expected) =
        cx.read_entity(&host, |host, _| (host.compact_height, host.loose_height));
    assert_eq!(icon.size.height, expected);
    assert_eq!(text.size.height, expected);
    assert_eq!(
        icon.size.height,
        compact_height + space::S6 * 2.0 - space::S2 * 2.0
    );
}

struct ScrollButtonHost {
    scroll: ScrollHandle,
    button_clicks: Rc<Cell<usize>>,
    row_clicks: Rc<Cell<usize>>,
}

impl Render for ScrollButtonHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let button_clicks = self.button_clicks.clone();
        let row_clicks = self.row_clicks.clone();
        div()
            .id("scroll-button-host")
            .w(px(200.0))
            .h(px(100.0))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                div()
                    .id("button-row")
                    .on_click(move |_, _, _| row_clicks.set(row_clicks.get() + 1))
                    .child(
                        div().debug_selector(|| "scrollable-button".into()).child(
                            Button::text("scrollable-button", "移除").on_click(move |_, _, _| {
                                button_clicks.set(button_clicks.get() + 1)
                            }),
                        ),
                    ),
            )
            .child(div().h(px(1000.0)).child("列表内容"))
    }
}

#[gpui::test]
fn button_keeps_click_isolation_but_allows_parent_scrolling(cx: &mut TestAppContext) {
    let button_clicks = Rc::new(Cell::new(0));
    let row_clicks = Rc::new(Cell::new(0));
    let scroll = ScrollHandle::new();
    let (_, cx) = cx.add_window_view(|_, _| ScrollButtonHost {
        scroll: scroll.clone(),
        button_clicks: button_clicks.clone(),
        row_clicks: row_clicks.clone(),
    });
    let button = cx
        .debug_bounds("scrollable-button")
        .expect("按钮应参与布局");
    cx.simulate_click(button.center(), gpui::Modifiers::default());
    assert_eq!(button_clicks.get(), 1);
    assert_eq!(row_clicks.get(), 0, "行内按钮不能触发所在行的点击");
    cx.simulate_event(ScrollWheelEvent {
        position: button.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(-20.0))),
        ..Default::default()
    });
    cx.run_until_parked();
    assert!(
        scroll.offset().y < Pixels::ZERO,
        "按钮不能遮蔽父容器的滚轮事件"
    );
    let moved = cx
        .debug_bounds("scrollable-button")
        .expect("滚动后仍应测量按钮");
    assert!(moved.top() < button.top(), "滚动后按钮实际位置应上移");
    assert_eq!(button_clicks.get(), 1, "滚动不能触发按钮点击");
}
