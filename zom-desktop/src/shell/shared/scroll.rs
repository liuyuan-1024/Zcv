//! 共享滚动条样式。
//!
//! 参考 Zed 的边界：具体 panel/runtime 持有滚动状态，滚动由 GPUI 的
//! list/scroll handle 管理；本模块只读取 handle 并绘制统一样式的滚动条。
//! 第一版不处理拖拽。

use std::cell::Cell;
use std::rc::Rc;

use gpui::{AnyElement, UniformListScrollHandle, div, prelude::*, px};

use crate::shell::shared::theme::{color, radius};

#[derive(Clone, Debug)]
pub(crate) struct ScrollHandle {
    inner: UniformListScrollHandle,
    last_revealed_item: Rc<Cell<Option<usize>>>,
}

impl ScrollHandle {
    pub(crate) fn new() -> Self {
        Self {
            inner: UniformListScrollHandle::new(),
            last_revealed_item: Rc::new(Cell::new(None)),
        }
    }

    pub(crate) fn reveal_item_if_changed(&self, index: usize) {
        if self.last_revealed_item.replace(Some(index)) != Some(index) {
            self.inner
                .scroll_to_item(index, gpui::ScrollStrategy::Center);
        }
    }

    pub(crate) fn inner(&self) -> UniformListScrollHandle {
        self.inner.clone()
    }
}

impl Default for ScrollHandle {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn scrollbar(handle: &ScrollHandle) -> AnyElement {
    let state = handle.inner.0.borrow();
    let Some(size) = state.last_item_size else {
        return div().id("shared-scrollbar").into_any_element();
    };
    let viewport_height = size.item.height;
    let content_height = size.contents.height;
    if viewport_height <= px(0.) || content_height <= viewport_height {
        return div().id("shared-scrollbar").into_any_element();
    }

    let scroll_top = -state.base_handle.offset().y;
    let max_scroll = (content_height - viewport_height).max(px(1.));
    let track_height = viewport_height;
    let thumb_height = (viewport_height / content_height * track_height)
        .max(px(18.0))
        .min(track_height);
    let thumb_top = (scroll_top / max_scroll) * (track_height - thumb_height);

    div()
        .id("shared-scrollbar")
        .absolute()
        .top(thumb_top)
        .right_0()
        .w(px(4.0))
        .h(thumb_height)
        .rounded(radius::full())
        .bg(color::gray::s05())
        .into_any_element()
}
