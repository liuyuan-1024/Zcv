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
    /// 覆盖在正文顶部的悬浮区域；自动滚动必须把目标行放到它下方。
    top_inset: Pixels,
}

/// 垂直滚动轴 thumb 的三态。
///
/// 状态跨帧持久存于 ScrollManager（滚动状态归属 Editor），每帧由EditorElement 读取决定绘制颜色与事件分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ScrollbarThumbState {
    #[default]
    Idle,
    Hovered,
    Dragging,
}

/// 待应用的自动滚动请求。
#[derive(Debug, Clone, Copy, PartialEq)]
enum PendingAutoscroll {
    /// 最小滚动：目标行进出视口才滚动（正常编辑跟随）。
    Fit(DisplayPoint),
    /// 顶部相对定位：目标行固定在视口顶部下方指定行数（导航跳转）。
    TopRelative {
        point: DisplayPoint,
        offset_rows: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ScrollManager {
    anchor: DisplayPoint,
    offset: Point<Pixels>,
    viewport: Option<ScrollViewport>,
    pending_autoscroll: Option<PendingAutoscroll>,
    /// 本次自动滚动请求的水平部分待布局后钳制（垂直部分已在布局前应用）。
    pending_horizontal_autoscroll: bool,
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
        top_inset: Pixels,
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
            top_inset: top_inset.max(Pixels::ZERO).min(height.max(Pixels::ZERO)),
        });

        let old_anchor = self.anchor;
        let old_offset = self.offset;
        self.set_scroll_left(self.offset.x);
        self.set_scroll_top(self.scroll_top());
        if let Some(pending) = self.pending_autoscroll {
            self.apply_autoscroll(pending);
        }
        self.anchor != old_anchor || self.offset != old_offset
    }

    pub(super) fn scroll_by(&mut self, delta: Point<Pixels>) -> bool {
        let old_anchor = self.anchor;
        let old_offset = self.offset;
        self.pending_autoscroll = None;
        self.pending_horizontal_autoscroll = false;

        self.set_scroll_left(self.offset.x - delta.x);
        self.set_scroll_top(self.scroll_top() - delta.y);

        self.anchor != old_anchor || self.offset != old_offset
    }

    pub(super) fn request_autoscroll(&mut self, point: DisplayPoint) {
        self.pending_autoscroll = Some(PendingAutoscroll::Fit(point));
    }

    /// 顶部相对定位：目标行固定在视口顶部下方指定行数。
    pub(super) fn request_scroll_to_top(&mut self, point: DisplayPoint, offset_rows: usize) {
        self.pending_autoscroll = Some(PendingAutoscroll::TopRelative { point, offset_rows });
    }

    fn apply_autoscroll(&mut self, pending: PendingAutoscroll) {
        match pending {
            PendingAutoscroll::Fit(point) => self.ensure_visible(point),
            PendingAutoscroll::TopRelative { point, offset_rows } => {
                let Some(viewport) = self.viewport else {
                    return;
                };
                let row_top = viewport.line_height * point.row().get();
                self.set_scroll_top(
                    row_top - viewport.top_inset - viewport.line_height * offset_rows,
                );
            }
        }
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
        self.pending_horizontal_autoscroll = false;
        self.set_scroll_top(scroll_top);
        self.anchor != old_anchor || self.offset != old_offset
    }

    /// 在组合文档结构刷新后恢复已经重新解析到当前投影的锚点。
    pub(super) fn restore_anchor(&mut self, anchor: DisplayPoint, offset: Point<Pixels>) -> bool {
        let changed = self.anchor != anchor || self.offset != offset;
        self.anchor = anchor;
        self.offset = offset;
        self.pending_autoscroll = None;
        self.pending_horizontal_autoscroll = false;
        changed
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

    /// 布局前调用：消费待自动滚动点并只应用垂直部分（光标行进出视口的锚点修正）。
    ///
    /// 垂直部分只依赖光标行与视口几何，不依赖布局；
    /// 在布局前应用可让首遍布局即为最终布局，避免光标移动帧的第二遍全量重排。
    pub(super) fn apply_pending_autoscroll_vertical(&mut self) -> bool {
        // 视口未就绪（首帧布局前）时保留请求，由布局时的 update_viewport 应用；
        // 否则 take 会吞掉请求导致导航定位丢失。
        if self.viewport.is_none() {
            return false;
        }
        let Some(pending) = self.pending_autoscroll.take() else {
            return false;
        };
        // 本次请求的水平部分留给布局后钳制（需要光标像素坐标）。
        self.pending_horizontal_autoscroll = true;
        let old_anchor = self.anchor;
        let old_offset = self.offset;
        self.apply_autoscroll(pending);
        self.anchor != old_anchor || self.offset != old_offset
    }

    /// 布局后调用：若本次有自动滚动请求则做水平钳制（光标 x 进出视口时平移），返回是否变化。
    ///
    /// 水平滚动只改变 offset.x，调用方对已算好的布局做平移而非重排；
    /// 手动滚动已清除请求，不会误触发。
    pub(super) fn complete_autoscroll_horizontal(
        &mut self,
        caret_left: Option<Pixels>,
        caret_right: Option<Pixels>,
    ) -> bool {
        if !self.pending_horizontal_autoscroll {
            return false;
        }
        self.pending_horizontal_autoscroll = false;
        let old_offset = self.offset;
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
        self.offset != old_offset
    }

    fn ensure_visible(&mut self, point: DisplayPoint) {
        let Some(viewport) = self.viewport else {
            return;
        };
        let scroll_top = self.scroll_top();
        let row_top = viewport.line_height * point.row().get();
        let row_bottom = row_top + viewport.line_height;
        let viewport_top = scroll_top + viewport.top_inset;
        let viewport_bottom = scroll_top + viewport.height;

        if row_top < viewport_top {
            self.set_scroll_top(row_top - viewport.top_inset);
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
            pending_horizontal_autoscroll: false,
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
        manager.update_viewport(100, px(100.), px(100.), px(200.), px(20.), px(0.));

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
        manager.update_viewport(50, px(100.), px(100.), px(200.), px(20.), px(0.));

        assert_eq!(manager.anchor().row(), DisplayRow::new(16));
        assert_eq!(manager.offset().y, px(0.));

        manager.request_autoscroll(DisplayPoint::new(DisplayRow::new(2), DisplayColumn::ZERO));
        manager.update_viewport(50, px(100.), px(100.), px(200.), px(20.), px(0.));
        assert_eq!(manager.anchor().row(), DisplayRow::new(2));
        assert_eq!(manager.offset().y, px(0.));
    }

    #[test]
    fn autoscroll_keeps_target_below_sticky_header() {
        let mut manager = ScrollManager::default();
        manager.update_viewport(50, px(100.), px(100.), px(200.), px(20.), px(40.));
        manager.scroll_to(px(200.));

        manager.request_autoscroll(DisplayPoint::new(DisplayRow::new(10), DisplayColumn::ZERO));
        assert!(manager.apply_pending_autoscroll_vertical());
        assert_eq!(manager.scroll_top(), px(160.));

        manager.request_scroll_to_top(
            DisplayPoint::new(DisplayRow::new(10), DisplayColumn::ZERO),
            2,
        );
        assert!(manager.apply_pending_autoscroll_vertical());
        assert_eq!(manager.scroll_top(), px(120.));
    }

    #[test]
    fn page_scroll_moves_one_visible_page_with_one_row_overlap() {
        let mut manager = ScrollManager::default();
        manager.update_viewport(100, px(100.), px(100.), px(200.), px(20.), px(0.));

        assert_eq!(manager.page_row_count(), Some(4));
        assert!(manager.scroll_page(true));
        assert_eq!(manager.anchor().row(), DisplayRow::new(4));
        assert!(manager.scroll_page(false));
        assert_eq!(manager.anchor().row(), DisplayRow::ZERO);
    }

    #[test]
    fn viewport_resize_clamps_existing_scroll_position() {
        let mut manager = ScrollManager::default();
        manager.update_viewport(10, px(100.), px(40.), px(200.), px(20.), px(0.));
        manager.scroll_by(point(px(-12.), px(-500.)));
        assert_eq!(manager.offset().x, px(12.));
        assert_eq!(manager.anchor().row(), DisplayRow::new(8));

        manager.update_viewport(3, px(100.), px(100.), px(200.), px(20.), px(0.));
        assert_eq!(manager.anchor().row(), DisplayRow::ZERO);
        assert_eq!(manager.offset().y, px(0.));
        assert_eq!(manager.offset().x, px(12.));
    }

    #[test]
    fn horizontal_scroll_is_clamped_to_content_width() {
        let mut manager = ScrollManager::default();
        manager.update_viewport(1, px(100.), px(40.), px(260.), px(20.), px(0.));

        manager.scroll_by(point(px(-10_000.), px(0.)));
        assert_eq!(manager.offset().x, px(160.));

        manager.scroll_by(point(px(10_000.), px(0.)));
        assert_eq!(manager.offset().x, px(0.));

        manager.scroll_by(point(px(-10_000.), px(0.)));
        manager.update_viewport(1, px(180.), px(40.), px(220.), px(20.), px(0.));
        assert_eq!(manager.offset().x, px(40.));
    }

    #[test]
    fn caret_autoscroll_reveals_exact_bounds_without_affecting_manual_scroll() {
        let mut manager = ScrollManager::default();
        manager.update_viewport(1, px(100.), px(40.), px(300.), px(20.), px(0.));
        manager.request_autoscroll(DisplayPoint::ZERO);

        // 垂直部分布局前应用（光标行在视口内，无变化）；水平部分布局后钳制。
        assert!(!manager.apply_pending_autoscroll_vertical());
        assert!(manager.complete_autoscroll_horizontal(Some(px(180.)), Some(px(182.))));
        assert_eq!(manager.offset().x, px(82.));

        // 手动滚动清除自动滚动请求，水平钳制不再触发。
        manager.scroll_by(point(px(-20.), px(0.)));
        assert_eq!(manager.offset().x, px(102.));
        assert!(!manager.complete_autoscroll_horizontal(Some(px(180.)), Some(px(182.))));
        assert_eq!(manager.offset().x, px(102.));
    }

    #[test]
    fn scroll_to_clamps_and_normalizes_anchor_and_offset() {
        let mut manager = ScrollManager::default();
        manager.update_viewport(100, px(100.), px(100.), px(200.), px(20.), px(0.));

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
