//! Editor 视图滚动状态。

use gpui::{Pixels, Point, point, px};

use super::display_map::{DisplayPoint, DisplayRow};

#[derive(Debug, Clone, Copy, PartialEq)]
struct ScrollViewport {
    line_count: usize,
    width: Pixels,
    height: Pixels,
    content_width: Pixels,
    line_height: Pixels,
}

/// 垂直滚动轴 thumb 的三态。
///
/// 状态跨帧持久存于 ScrollManager（滚动状态归属 Editor），每帧由EditorElement 读取决定绘制颜色与事件分支（对齐 Zed 的 ScrollbarThumbState）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ScrollbarThumbState {
    #[default]
    Idle,
    Hovered,
    Dragging,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ScrollManager {
    anchor: DisplayPoint,
    offset: Point<Pixels>,
    viewport: Option<ScrollViewport>,
    pending_autoscroll: Option<DisplayPoint>,
    thumb_state: ScrollbarThumbState,
}

impl ScrollManager {
    pub(super) fn anchor(&self) -> DisplayPoint {
        self.anchor
    }

    pub(super) fn offset(&self) -> Point<Pixels> {
        self.offset
    }

    pub(super) fn update_viewport(
        &mut self,
        line_count: usize,
        width: Pixels,
        height: Pixels,
        content_width: Pixels,
        line_height: Pixels,
    ) -> bool {
        if line_height <= Pixels::ZERO {
            return false;
        }
        self.viewport = Some(ScrollViewport {
            line_count: line_count.max(1),
            width: width.max(Pixels::ZERO),
            height: height.max(Pixels::ZERO),
            content_width: content_width.max(Pixels::ZERO),
            line_height,
        });

        let old_anchor = self.anchor;
        let old_offset = self.offset;
        self.set_scroll_left(self.offset.x);
        self.set_scroll_top(self.scroll_top());
        if let Some(point) = self.pending_autoscroll {
            self.ensure_visible(point);
        }
        self.anchor != old_anchor || self.offset != old_offset
    }

    pub(super) fn scroll_by(&mut self, delta: Point<Pixels>) -> bool {
        let old_anchor = self.anchor;
        let old_offset = self.offset;
        self.pending_autoscroll = None;

        self.set_scroll_left(self.offset.x - delta.x);
        self.set_scroll_top(self.scroll_top() - delta.y);

        self.anchor != old_anchor || self.offset != old_offset
    }

    pub(super) fn request_autoscroll(&mut self, point: DisplayPoint) {
        self.pending_autoscroll = Some(point);
    }

    pub(super) fn page_row_count(&self) -> Option<usize> {
        let viewport = self.viewport?;
        let visible_rows = (viewport.height / viewport.line_height).floor() as usize;
        Some(visible_rows.saturating_sub(1).max(1))
    }

    pub(super) fn scroll_page(&mut self, down: bool) -> bool {
        let viewport = match self.viewport {
            Some(viewport) => viewport,
            None => return false,
        };
        let distance = viewport.line_height * self.page_row_count().unwrap_or(1);
        let delta = if down {
            point(Pixels::ZERO, -distance)
        } else {
            point(Pixels::ZERO, distance)
        };
        self.scroll_by(delta)
    }

    /// 可见区顶部滚动量（像素）。
    pub(super) fn scroll_top(&self) -> Pixels {
        let Some(viewport) = self.viewport else {
            return self.offset.y;
        };
        viewport.line_height * self.anchor.row().get() + self.offset.y
    }

    /// 可滚动上界：内容总高 − 视口高；未设置视口时为 0。
    pub(super) fn max_scroll_top(&self) -> Pixels {
        self.viewport.map_or(Pixels::ZERO, |viewport| {
            (viewport.line_height * viewport.line_count - viewport.height).max(Pixels::ZERO)
        })
    }

    /// 绝对滚动到指定顶部位置：清除待自动滚动，钳制到 [0, max_scroll_top]。
    /// 返回是否发生变化（供 Editor 包装层决定是否 notify）。
    pub(super) fn scroll_to(&mut self, scroll_top: Pixels) -> bool {
        let old_anchor = self.anchor;
        let old_offset = self.offset;
        self.pending_autoscroll = None;
        self.set_scroll_top(scroll_top);
        self.anchor != old_anchor || self.offset != old_offset
    }

    /// 滚动轴 thumb 当前三态。
    pub(super) fn thumb_state(&self) -> ScrollbarThumbState {
        self.thumb_state
    }

    /// 置悬停态，返回是否发生变化。
    pub(super) fn set_thumb_hovered(&mut self) -> bool {
        self.update_thumb_state(ScrollbarThumbState::Hovered)
    }

    /// 置拖动态，返回是否发生变化。
    pub(super) fn set_thumb_dragged(&mut self) -> bool {
        self.update_thumb_state(ScrollbarThumbState::Dragging)
    }

    /// 复位为 Idle，返回是否发生变化。
    pub(super) fn reset_thumb_state(&mut self) -> bool {
        self.update_thumb_state(ScrollbarThumbState::Idle)
    }

    pub(super) fn complete_autoscroll(
        &mut self,
        caret_left: Option<Pixels>,
        caret_right: Option<Pixels>,
    ) -> bool {
        let Some(point) = self.pending_autoscroll.take() else {
            return false;
        };
        let old_anchor = self.anchor;
        let old_offset = self.offset;
        self.ensure_visible(point);

        if let (Some(viewport), Some(caret_left), Some(caret_right)) =
            (self.viewport, caret_left, caret_right)
        {
            let visible_left = self.offset.x;
            let visible_right = self.offset.x + viewport.width;
            if caret_left < visible_left {
                self.set_scroll_left(caret_left);
            } else if caret_right > visible_right {
                self.set_scroll_left(caret_right - viewport.width);
            }
        }

        self.anchor != old_anchor || self.offset != old_offset
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

    fn set_scroll_left(&mut self, scroll_left: Pixels) {
        let maximum = self
            .viewport
            .map(|viewport| (viewport.content_width - viewport.width).max(Pixels::ZERO));
        self.offset.x = match maximum {
            Some(maximum) => scroll_left.max(Pixels::ZERO).min(maximum),
            None => scroll_left.max(Pixels::ZERO),
        };
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

    fn update_thumb_state(&mut self, state: ScrollbarThumbState) -> bool {
        if self.thumb_state != state {
            self.thumb_state = state;
            true
        } else {
            false
        }
    }
}

impl Default for ScrollManager {
    fn default() -> Self {
        Self {
            anchor: DisplayPoint::ZERO,
            offset: point(px(0.0), px(0.0)),
            viewport: None,
            pending_autoscroll: None,
            thumb_state: ScrollbarThumbState::Idle,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::display_map::DisplayColumn;

    use super::*;

    #[test]
    fn wheel_delta_normalizes_anchor_and_clamps_document_edges() {
        let mut manager = ScrollManager::default();
        manager.update_viewport(100, px(100.), px(100.), px(200.), px(20.));

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
        manager.update_viewport(50, px(100.), px(100.), px(200.), px(20.));

        assert_eq!(manager.anchor().row(), DisplayRow::new(16));
        assert_eq!(manager.offset().y, px(0.));

        manager.request_autoscroll(DisplayPoint::new(DisplayRow::new(2), DisplayColumn::ZERO));
        manager.update_viewport(50, px(100.), px(100.), px(200.), px(20.));
        assert_eq!(manager.anchor().row(), DisplayRow::new(2));
        assert_eq!(manager.offset().y, px(0.));
    }

    #[test]
    fn page_scroll_moves_one_visible_page_with_one_row_overlap() {
        let mut manager = ScrollManager::default();
        manager.update_viewport(100, px(100.), px(100.), px(200.), px(20.));

        assert_eq!(manager.page_row_count(), Some(4));
        assert!(manager.scroll_page(true));
        assert_eq!(manager.anchor().row(), DisplayRow::new(4));
        assert!(manager.scroll_page(false));
        assert_eq!(manager.anchor().row(), DisplayRow::ZERO);
    }

    #[test]
    fn viewport_resize_clamps_existing_scroll_position() {
        let mut manager = ScrollManager::default();
        manager.update_viewport(10, px(100.), px(40.), px(200.), px(20.));
        manager.scroll_by(point(px(-12.), px(-500.)));
        assert_eq!(manager.offset().x, px(12.));
        assert_eq!(manager.anchor().row(), DisplayRow::new(8));

        manager.update_viewport(3, px(100.), px(100.), px(200.), px(20.));
        assert_eq!(manager.anchor().row(), DisplayRow::ZERO);
        assert_eq!(manager.offset().y, px(0.));
        assert_eq!(manager.offset().x, px(12.));
    }

    #[test]
    fn horizontal_scroll_is_clamped_to_content_width() {
        let mut manager = ScrollManager::default();
        manager.update_viewport(1, px(100.), px(40.), px(260.), px(20.));

        manager.scroll_by(point(px(-10_000.), px(0.)));
        assert_eq!(manager.offset().x, px(160.));

        manager.scroll_by(point(px(10_000.), px(0.)));
        assert_eq!(manager.offset().x, px(0.));

        manager.scroll_by(point(px(-10_000.), px(0.)));
        manager.update_viewport(1, px(180.), px(40.), px(220.), px(20.));
        assert_eq!(manager.offset().x, px(40.));
    }

    #[test]
    fn caret_autoscroll_reveals_exact_bounds_without_affecting_manual_scroll() {
        let mut manager = ScrollManager::default();
        manager.update_viewport(1, px(100.), px(40.), px(300.), px(20.));
        manager.request_autoscroll(DisplayPoint::ZERO);

        assert!(manager.complete_autoscroll(Some(px(180.)), Some(px(182.))));
        assert_eq!(manager.offset().x, px(82.));

        manager.scroll_by(point(px(-20.), px(0.)));
        assert_eq!(manager.offset().x, px(102.));
        assert!(!manager.complete_autoscroll(Some(px(180.)), Some(px(182.))));
        assert_eq!(manager.offset().x, px(102.));
    }

    #[test]
    fn scroll_to_clamps_and_normalizes_anchor_and_offset() {
        let mut manager = ScrollManager::default();
        manager.update_viewport(100, px(100.), px(100.), px(200.), px(20.));

        assert!(manager.scroll_to(px(35.)));
        assert_eq!(manager.anchor().row(), DisplayRow::new(1));
        assert_eq!(manager.offset().y, px(15.));
        assert_eq!(manager.scroll_top(), px(35.));

        assert!(manager.scroll_to(px(10_000.)));
        assert_eq!(manager.scroll_top(), px(1_900.));
        assert_eq!(manager.anchor().row(), DisplayRow::new(95));
        assert_eq!(manager.offset().y, px(0.));

        assert!(manager.scroll_to(px(-100.)));
        assert_eq!(manager.scroll_top(), px(0.));
        assert_eq!(manager.anchor(), DisplayPoint::ZERO);

        assert!(!manager.scroll_to(px(0.)));
    }

    #[test]
    fn scroll_to_without_viewport_writes_subpixel_offset() {
        let mut manager = ScrollManager::default();

        assert_eq!(manager.max_scroll_top(), px(0.));
        assert!(manager.scroll_to(px(42.)));
        assert_eq!(manager.offset().y, px(42.));
        assert_eq!(manager.scroll_top(), px(42.));
        assert!(!manager.scroll_to(px(42.)));
    }

    #[test]
    fn thumb_state_transitions_are_dirty_checked() {
        let mut manager = ScrollManager::default();

        assert_eq!(manager.thumb_state(), ScrollbarThumbState::Idle);
        assert!(manager.set_thumb_hovered());
        assert!(!manager.set_thumb_hovered());
        assert_eq!(manager.thumb_state(), ScrollbarThumbState::Hovered);
        assert!(manager.set_thumb_dragged());
        assert_eq!(manager.thumb_state(), ScrollbarThumbState::Dragging);
        assert!(manager.reset_thumb_state());
        assert_eq!(manager.thumb_state(), ScrollbarThumbState::Idle);
        assert!(!manager.reset_thumb_state());
    }
}
