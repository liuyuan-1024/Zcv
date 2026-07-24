//! Editor 视图滚动状态。

use gpui::{Pixels, Point, point, px};

use super::display_map::DisplayPoint;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ScrollManager {
    anchor: DisplayPoint,
    offset: Point<Pixels>,
}

impl ScrollManager {
    pub(super) fn anchor(&self) -> DisplayPoint {
        self.anchor
    }

    pub(super) fn set_anchor(&mut self, anchor: DisplayPoint) {
        self.anchor = anchor;
    }

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
            anchor: DisplayPoint::ZERO,
            offset: point(px(0.0), px(0.0)),
        }
    }
}
