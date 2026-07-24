//! Editor 视图滚动状态。

use gpui::{Pixels, Point, point, px};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ScrollManager {
    offset: Point<Pixels>,
}

impl ScrollManager {
    pub(super) fn offset(&self) -> Point<Pixels> {
        self.offset
    }

    pub(super) fn set_offset(&mut self, offset: Point<Pixels>) {
        self.offset = offset;
    }
}

impl Default for ScrollManager {
    fn default() -> Self {
        Self {
            offset: point(px(0.0), px(0.0)),
        }
    }
}
