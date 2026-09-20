use gpui::{Context, Render, TestAppContext, Window, prelude::*, px, size};

use super::*;

struct ShortRow;
impl Render for ShortRow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .child(ListItem::new("short").child("标题").subtitle("短路径"))
    }
}

struct LongRow;
impl Render for LongRow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // 文本远超任何测试窗口宽度，必然换行
        div().w_full().child(
            ListItem::new("long")
                .child("标题")
                .subtitle("这是一个非常长的路径，用来验证次行文本自动换行不会被截断：".repeat(500)),
        )
    }
}

/// 次行文本允许自动换行：行高随内容增长（变高列表按实际高度布局），
/// 长路径完整展示而不被裁剪。
#[gpui::test]
fn long_subtitle_grows_row_height(cx: &mut TestAppContext) {
    // 窗口调窄，保证长路径必然换行
    let (_, cx) = cx.add_window_view(|_, _| ShortRow);
    cx.simulate_window_resize(cx.windows()[0], size(px(360.0), px(400.0)));
    let short_height = cx
        .debug_bounds("list-item")
        .expect("短路径行应参与布局")
        .size
        .height;

    let (_, cx) = cx.add_window_view(|_, _| LongRow);
    cx.simulate_window_resize(cx.windows()[1], size(px(360.0), px(400.0)));
    let long_height = cx
        .debug_bounds("list-item")
        .expect("长路径行应参与布局")
        .size
        .height;

    assert!(
        long_height > short_height,
        "长路径应换行撑高行（不被截断）：短行 {short_height}，长行 {long_height}"
    );
}
