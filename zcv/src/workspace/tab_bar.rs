//! TabBar —— 标签栏容器组件。
//!
//! 负责渲染横向滚动标签区，接受 [`ScrollHandle`] 实现程序化滚动。
//! 底部始终有一条由 overlay `border_b_1` 绘制的连续底线，选中标签通过 `pb(1px)` 覆盖该线以形成与内容区连接的视觉效果。

use gpui::{AnyElement, Div, ScrollHandle, div, prelude::*};

use zcv_theme::color;

/// 标签栏容器，标签渲染由调用方提供子元素。
pub(crate) struct TabBar {
    scroll_handle: Option<ScrollHandle>,
}

impl TabBar {
    pub fn new() -> Self {
        Self {
            scroll_handle: None,
        }
    }

    /// 关联 ScrollHandle，使标签栏可程序化滚动。
    pub fn track_scroll(mut self, handle: &ScrollHandle) -> Self {
        self.scroll_handle = Some(handle.clone());
        self
    }

    /// 设置外层容器样式并填入子元素，然后渲染。
    ///
    /// 内部自动添加一个 absolute 定位的 `border_b_1` overlay 作为连续底线，tabs 渲染在其上层，选中标签通过 `pb(1px)` 覆盖该线。
    pub fn with_bar(
        self,
        f: impl FnOnce(Div) -> Div,
        children: impl IntoIterator<Item = AnyElement>,
    ) -> impl gpui::IntoElement {
        let border_color = color::current().gray.s[4];

        let outer = f(div());

        outer.child(
            div()
                .relative()
                .flex_1()
                .h_full()
                .overflow_x_hidden()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .border_b_1()
                        .border_color(border_color),
                )
                .child({
                    let mut scroll_area = div()
                        .id("tab-bar-scroll")
                        .flex()
                        .flex_row()
                        .flex_grow()
                        .overflow_x_scroll();
                    if let Some(ref handle) = self.scroll_handle {
                        scroll_area = scroll_area.track_scroll(handle);
                    }
                    scroll_area.children(children)
                }),
        )
    }
}

impl Default for TabBar {
    fn default() -> Self {
        Self::new()
    }
}
