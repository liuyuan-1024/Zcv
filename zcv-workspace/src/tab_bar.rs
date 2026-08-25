//! TabBar —— 标签栏容器组件。
//!
//! 负责渲染横向滚动标签区，接受 [`ScrollHandle`] 实现程序化滚动。
//! 标签区右侧提供固定功能插槽（不随标签滚动），插槽内容由构建器在每次渲染时生成。

use std::rc::Rc;

use gpui::{AnyElement, App, Div, ScrollHandle, div, prelude::*};
use zcv_theme::{color, space};

/// 标签栏右侧功能插槽的构建器：每次渲染时调用生成插槽元素。
/// 元素不可复制（ArenaBox 独占所有权），无法缓存实例，只能以构建器形式保存。
pub(crate) type TabBarTrailing = Rc<dyn Fn(&App) -> AnyElement>;

/// 标签栏容器，标签渲染由调用方提供子元素。
pub(crate) struct TabBar {
    scroll_handle: Option<ScrollHandle>,
    /// 右侧功能插槽的构建器。
    trailing: Option<TabBarTrailing>,
}

impl TabBar {
    pub(crate) fn new() -> Self {
        Self {
            scroll_handle: None,
            trailing: None,
        }
    }

    /// 关联 ScrollHandle，使标签栏可程序化滚动。
    pub(crate) fn track_scroll(mut self, handle: &ScrollHandle) -> Self {
        self.scroll_handle = Some(handle.clone());
        self
    }

    /// 设置右侧功能插槽的构建器（不随标签滚动）。
    pub(crate) fn with_trailing(mut self, build: TabBarTrailing) -> Self {
        self.trailing = Some(build);
        self
    }

    /// 设置外层容器样式并填入子元素，然后渲染。
    pub(crate) fn with_bar(
        self,
        cx: &gpui::App,
        f: impl FnOnce(Div) -> Div,
        children: impl IntoIterator<Item = AnyElement>,
    ) -> impl gpui::IntoElement {
        let border_color = color::current(cx).border;
        let trailing = self.trailing;
        let outer = f(div()).border_b_1().border_color(border_color);

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
                        .flex_grow()
                        .overflow_x_scroll();
                    if let Some(ref handle) = self.scroll_handle {
                        scroll_area = scroll_area.track_scroll(handle);
                    }
                    scroll_area.children(children)
                }),
        );
        if let Some(build) = trailing {
            bar = bar.child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .px(space::S6)
                    .child(build(cx)),
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
