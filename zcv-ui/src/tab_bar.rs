//! TabBar —— 标签栏容器组件。
//!
//! 负责渲染横向滚动标签区，接受 [`ScrollHandle`] 实现程序化滚动。
//! 标签区右侧提供固定功能插槽（不随标签滚动），插槽内容由构建器在每次渲染时生成。

use crate::Tab;
use gpui::{AnyElement, App, ScrollHandle, Window, div, prelude::*};
use zcv_theme::{color, scale};

/// 标签栏容器，标签渲染由调用方提供子元素。
pub struct TabBar {
    scroll_handle: Option<ScrollHandle>,
    trailing: Option<AnyElement>,
}

impl TabBar {
    pub fn new() -> Self {
        Self {
            scroll_handle: None,
            trailing: None,
        }
    }

    /// 关联 ScrollHandle，使标签栏可程序化滚动。
    pub fn track_scroll(mut self, handle: &ScrollHandle) -> Self {
        self.scroll_handle = Some(handle.clone());
        self
    }

    /// 设置右侧功能插槽的构建器（不随标签滚动）。
    pub fn with_trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing = Some(element.into_any_element());
        self
    }

    /// 渲染标签及固定尾部插槽。
    pub fn render(
        self,
        cx: &App,
        window: &Window,
        children: impl IntoIterator<Item = AnyElement>,
    ) -> impl gpui::IntoElement {
        let colors = color::current(cx);
        let trailing = self.trailing;
        let outer = div()
            .flex()
            .items_center()
            .flex_shrink_0()
            .bg(colors.tab_bar_background)
            .relative()
            .h(Tab::container_height(window, cx))
            .child(
                // 先绘制底线，活动标签的实色背景会覆盖对应区域。
                div()
                    .absolute()
                    .inset_0()
                    .border_b_1()
                    .border_color(colors.border),
            );

        let mut bar = outer.child(
            div()
                .relative()
                .flex_1()
                .h_full()
                .overflow_x_hidden()
                .child({
                    let mut scroll_area = div()
                        .id("tab-bar-scroll")
                        .flex()
                        .flex_row()
                        .flex_grow(1.0)
                        .overflow_x_scroll();
                    if let Some(ref handle) = self.scroll_handle {
                        scroll_area = scroll_area.track_scroll(handle);
                    }
                    scroll_area.children(children)
                }),
        );
        if let Some(trailing) = trailing {
            bar = bar.child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .px(scale::S6)
                    .child(trailing),
            );
        }
        bar
    }
}

impl Default for TabBar {
    fn default() -> Self {
        Self::new()
    }
}
