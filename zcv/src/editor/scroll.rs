//! Editor 视图滚动状态。

use gpui::{Pixels, Point, point, px};

use super::display_map::{DisplayPoint, DisplayRow};

#[derive(Debug, Clone, Copy, PartialEq)]
struct ScrollViewport {
    line_count: usize,
    height: Pixels,
    line_height: Pixels,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ScrollManager {
    anchor: DisplayPoint,
    offset: Point<Pixels>,
    viewport: Option<ScrollViewport>,
    pending_autoscroll: Option<DisplayPoint>,
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

    pub(super) fn update_viewport(
        &mut self,
        line_count: usize,
        height: Pixels,
        line_height: Pixels,
    ) -> bool {
        if line_height <= Pixels::ZERO {
            return false;
        }
        self.viewport = Some(ScrollViewport {
            line_count: line_count.max(1),
            height: height.max(Pixels::ZERO),
            line_height,
        });

        let old_anchor = self.anchor;
        let old_offset = self.offset;
        self.set_scroll_top(self.scroll_top());
        if let Some(point) = self.pending_autoscroll.take() {
            self.ensure_visible(point);
        }
        self.anchor != old_anchor || self.offset != old_offset
    }

    pub(super) fn scroll_by(&mut self, delta: Point<Pixels>) -> bool {
        let old_anchor = self.anchor;
        let old_offset = self.offset;
        self.pending_autoscroll = None;

        self.offset.x = (self.offset.x - delta.x).max(Pixels::ZERO);
        self.set_scroll_top(self.scroll_top() - delta.y);

        self.anchor != old_anchor || self.offset != old_offset
    }

    pub(super) fn request_autoscroll(&mut self, point: DisplayPoint) {
        self.pending_autoscroll = Some(point);
    }

    fn ensure_visible(&mut self, point: DisplayPoint) {
        let Some(viewport) = self.viewport else {
            return;
        };
        let scroll_top = self.scroll_top();
        let row_top = viewport.line_height * point.row().get();
        let row_bottom = row_top + viewport.line_height;
        let viewport_bottom = scroll_top + viewport.height;

        if row_top < scroll_top {
            self.set_scroll_top(row_top);
        } else if row_bottom > viewport_bottom {
            self.set_scroll_top(row_bottom - viewport.height);
        }
    }

    fn scroll_top(&self) -> Pixels {
        let Some(viewport) = self.viewport else {
            return self.offset.y;
        };
        viewport.line_height * self.anchor.row().get() + self.offset.y
    }

    fn set_scroll_top(&mut self, scroll_top: Pixels) {
        let Some(viewport) = self.viewport else {
            self.offset.y = scroll_top.max(Pixels::ZERO);
            return;
        };
        let content_height = viewport.line_height * viewport.line_count;
        let maximum = (content_height - viewport.height).max(Pixels::ZERO);
        let scroll_top = scroll_top.max(Pixels::ZERO).min(maximum);
        let row = ((scroll_top / viewport.line_height).floor() as usize)
            .min(viewport.line_count.saturating_sub(1));

        self.anchor = DisplayPoint::new(DisplayRow::new(row), self.anchor.column());
        self.offset.y = scroll_top - viewport.line_height * row;
    }
}

impl Default for ScrollManager {
    fn default() -> Self {
        Self {
            anchor: DisplayPoint::ZERO,
            offset: point(px(0.0), px(0.0)),
            viewport: None,
            pending_autoscroll: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use zcv_engine::DisplayColumn;

    use super::*;

    #[test]
    fn wheel_delta_normalizes_anchor_and_clamps_document_edges() {
        let mut manager = ScrollManager::default();
        manager.update_viewport(100, px(100.), px(20.));

        assert!(manager.scroll_by(point(px(0.), px(-55.))));
        assert_eq!(
            manager.anchor(),
            DisplayPoint::new(DisplayRow::new(2), DisplayColumn::ZERO)
        );
        assert_eq!(manager.offset().y, px(15.));

        manager.scroll_by(point(px(0.), px(-10_000.)));
        assert_eq!(manager.anchor().row(), DisplayRow::new(95));
        assert_eq!(manager.offset().y, px(0.));

        manager.scroll_by(point(px(0.), px(10_000.)));
        assert_eq!(manager.anchor(), DisplayPoint::ZERO);
        assert_eq!(manager.offset(), point(px(0.), px(0.)));
    }

    #[test]
    fn pending_autoscroll_reveals_rows_after_viewport_update() {
        let mut manager = ScrollManager::default();
        manager.request_autoscroll(DisplayPoint::new(
            DisplayRow::new(20),
            DisplayColumn::new(4),
        ));
        manager.update_viewport(50, px(100.), px(20.));

        assert_eq!(manager.anchor().row(), DisplayRow::new(16));
        assert_eq!(manager.offset().y, px(0.));

        manager.request_autoscroll(DisplayPoint::new(DisplayRow::new(2), DisplayColumn::ZERO));
        manager.update_viewport(50, px(100.), px(20.));
        assert_eq!(manager.anchor().row(), DisplayRow::new(2));
        assert_eq!(manager.offset().y, px(0.));
    }

    #[test]
    fn viewport_resize_clamps_existing_scroll_position() {
        let mut manager = ScrollManager::default();
        manager.update_viewport(10, px(40.), px(20.));
        manager.scroll_by(point(px(-12.), px(-500.)));
        assert_eq!(manager.offset().x, px(12.));
        assert_eq!(manager.anchor().row(), DisplayRow::new(8));

        manager.update_viewport(3, px(100.), px(20.));
        assert_eq!(manager.anchor().row(), DisplayRow::ZERO);
        assert_eq!(manager.offset().y, px(0.));
        assert_eq!(manager.offset().x, px(12.));
    }
}
