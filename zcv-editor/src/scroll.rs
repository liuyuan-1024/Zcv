//! Editor 视图滚动状态。

use zcv_multi_buffer::MultiBufferAnchor;

use gpui::{Pixels, Point, point, px};
use zcv_text::Affinity;

use super::display_map::{DisplayColumn, DisplayPoint, DisplayRow, DisplaySnapshot};

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
/// 状态跨帧持久存于 ScrollManager（滚动状态归属 Editor），每帧由 EditorElement 读取决定绘制颜色与事件分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ScrollbarThumbState {
    #[default]
    Idle,
    Hovered,
    Dragging,
}

/// 长期滚动位置：组合锚点 + 锚点行内像素余量。
///
/// 显示行与像素位置在消费时按当前 DisplaySnapshot 解析；不长期保存 DisplayPoint。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ScrollAnchor {
    pub(super) anchor: MultiBufferAnchor,
    pub(super) offset: Point<Pixels>,
}

impl ScrollAnchor {
    fn new() -> Self {
        Self {
            anchor: MultiBufferAnchor::Min,
            offset: point(px(0.0), px(0.0)),
        }
    }
}

/// 待应用的自动滚动请求。
///
/// 目标以组合锚点保存，显示点（DisplayPoint）在应用时按当前布局换算：
/// 软换行宽度在首帧布局时才确定，提前换算会把换行前的显示行号固化进请求，导致长行文件导航时滚动不到位。
#[derive(Debug, Clone, Copy, PartialEq)]
enum PendingAutoscroll {
    /// 最小滚动：目标行进出视口才滚动（正常编辑跟随）。
    Fit(MultiBufferAnchor),
    /// 顶部相对定位：目标行固定在视口顶部下方指定行数（导航跳转）。
    TopRelative {
        anchor: MultiBufferAnchor,
        offset_rows: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ScrollManager {
    /// 权威长期位置。
    anchor: ScrollAnchor,
    /// 当前帧派生显示点；由锚点按快照解析，供不持有快照的读取方（布局、滚动条几何）使用。
    display_point: DisplayPoint,
    viewport: Option<ScrollViewport>,
    pending_autoscroll: Option<PendingAutoscroll>,
    /// 本次自动滚动请求的水平部分待布局后钳制（垂直部分已在布局前应用）。
    pending_horizontal_autoscroll: bool,
    thumb_state: ScrollbarThumbState,
}

impl ScrollManager {
    /// 当前帧派生显示点。
    pub(super) fn anchor(&self) -> DisplayPoint {
        self.display_point
    }

    pub(super) fn offset(&self) -> Point<Pixels> {
        self.anchor.offset
    }

    /// 按当前快照从权威锚点重新解析派生显示点。
    pub(super) fn refresh(&mut self, snapshot: &DisplaySnapshot) {
        if let Some(point) = resolve_display_point(snapshot, &self.anchor.anchor) {
            self.display_point = point;
        }
    }

    pub(super) fn update_viewport(
        &mut self,
        line_count: usize,
        width: Pixels,
        height: Pixels,
        content_width: Pixels,
        line_height: Pixels,
        top_inset: Pixels,
        snapshot: &DisplaySnapshot,
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
        let old_point = self.display_point;
        self.set_scroll_left(self.anchor.offset.x);
        self.set_scroll_top(self.scroll_top(), snapshot);
        self.anchor != old_anchor || self.display_point != old_point
    }

    pub(super) fn scroll_by(&mut self, delta: Point<Pixels>, snapshot: &DisplaySnapshot) -> bool {
        let old_anchor = self.anchor;
        let old_point = self.display_point;
        self.pending_autoscroll = None;
        self.pending_horizontal_autoscroll = false;

        self.set_scroll_left(self.anchor.offset.x - delta.x);
        self.set_scroll_top(self.scroll_top() - delta.y, snapshot);

        self.anchor != old_anchor || self.display_point != old_point
    }

    pub(super) fn request_autoscroll(&mut self, anchor: MultiBufferAnchor) {
        self.pending_autoscroll = Some(PendingAutoscroll::Fit(anchor));
    }

    /// 顶部相对定位：目标锚点固定在视口顶部下方指定行数。
    pub(super) fn request_scroll_to_top(&mut self, anchor: MultiBufferAnchor, offset_rows: usize) {
        self.pending_autoscroll = Some(PendingAutoscroll::TopRelative {
            anchor,
            offset_rows,
        });
    }

    /// 应用待自动滚动的垂直部分；目标显示点由锚点按当前布局快照解析。
    fn apply_autoscroll(&mut self, pending: PendingAutoscroll, snapshot: &DisplaySnapshot) {
        match pending {
            PendingAutoscroll::Fit(anchor) => {
                if let Some(point) = resolve_display_point(snapshot, &anchor) {
                    self.ensure_visible(point, snapshot);
                }
            }
            PendingAutoscroll::TopRelative {
                anchor,
                offset_rows,
            } => {
                let Some(viewport) = self.viewport else {
                    return;
                };
                let Some(point) = resolve_display_point(snapshot, &anchor) else {
                    return;
                };
                let row_top = viewport.line_height * point.row().get();
                self.set_scroll_top(
                    row_top - viewport.top_inset - viewport.line_height * offset_rows,
                    snapshot,
                );
            }
        }
    }

    pub(super) fn page_row_count(&self) -> Option<usize> {
        let viewport = self.viewport?;
        let visible_rows = (viewport.height / viewport.line_height).floor() as usize;
        Some(visible_rows.saturating_sub(1).max(1))
    }

    pub(super) fn scroll_page(&mut self, down: bool, snapshot: &DisplaySnapshot) -> bool {
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
        self.scroll_by(delta, snapshot)
    }

    /// 可见区顶部滚动量（像素）。
    pub(super) fn scroll_top(&self) -> Pixels {
        let Some(viewport) = self.viewport else {
            return self.anchor.offset.y;
        };
        viewport.line_height * self.display_point.row().get() + self.anchor.offset.y
    }

    /// 可滚动上界：内容总高 − 视口高；未设置视口时为 0。
    pub(super) fn max_scroll_top(&self) -> Pixels {
        self.viewport.map_or(Pixels::ZERO, |viewport| {
            (viewport.line_height * viewport.line_count - viewport.height).max(Pixels::ZERO)
        })
    }

    /// 绝对滚动到指定顶部位置：清除待自动滚动，钳制到 [0, max_scroll_top]。
    /// 返回是否发生变化（供 Editor 包装层决定是否 notify）。
    pub(super) fn scroll_to(&mut self, scroll_top: Pixels, snapshot: &DisplaySnapshot) -> bool {
        let old_anchor = self.anchor;
        let old_point = self.display_point;
        self.pending_autoscroll = None;
        self.pending_horizontal_autoscroll = false;
        self.set_scroll_top(scroll_top, snapshot);
        self.anchor != old_anchor || self.display_point != old_point
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
    /// 目标显示点按当前布局快照解析；软换行重排发生在布局前，因此这里换算出的行号与最终布局一致。
    /// 垂直部分只依赖光标行与视口几何，不依赖布局；在布局前应用可让首遍布局即为最终布局。
    pub(super) fn apply_pending_autoscroll_vertical(&mut self, snapshot: &DisplaySnapshot) -> bool {
        // 视口未就绪（首帧布局前）时保留请求，由布局时的 update_viewport 设置视口后再次应用；
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
        let old_point = self.display_point;
        self.apply_autoscroll(pending, snapshot);
        self.anchor != old_anchor || self.display_point != old_point
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
        let old_offset = self.anchor.offset;
        if let (Some(viewport), Some(caret_left), Some(caret_right)) =
            (self.viewport, caret_left, caret_right)
        {
            let visible_left = self.anchor.offset.x;
            let visible_right = self.anchor.offset.x + viewport.width;
            if caret_left < visible_left {
                self.set_scroll_left(caret_left);
            } else if caret_right > visible_right {
                self.set_scroll_left(caret_right - viewport.width);
            }
        }
        self.anchor.offset != old_offset
    }

    fn ensure_visible(&mut self, point: DisplayPoint, snapshot: &DisplaySnapshot) {
        let Some(viewport) = self.viewport else {
            return;
        };
        let scroll_top = self.scroll_top();
        let row_top = viewport.line_height * point.row().get();
        let row_bottom = row_top + viewport.line_height;
        let viewport_top = scroll_top + viewport.top_inset;
        let viewport_bottom = scroll_top + viewport.height;

        if row_top < viewport_top {
            self.set_scroll_top(row_top - viewport.top_inset, snapshot);
        } else if row_bottom > viewport_bottom {
            self.set_scroll_top(row_bottom - viewport.height, snapshot);
        }
    }

    fn set_scroll_left(&mut self, scroll_left: Pixels) {
        let maximum = self
            .viewport
            .map(|viewport| (viewport.content_width - viewport.width).max(Pixels::ZERO));
        self.anchor.offset.x = match maximum {
            Some(maximum) => scroll_left.max(Pixels::ZERO).min(maximum),
            None => scroll_left.max(Pixels::ZERO),
        };
    }

    fn set_scroll_top(&mut self, scroll_top: Pixels, snapshot: &DisplaySnapshot) {
        let Some(viewport) = self.viewport else {
            self.anchor.offset.y = scroll_top.max(Pixels::ZERO);
            return;
        };
        let content_height = viewport.line_height * viewport.line_count;
        let maximum = (content_height - viewport.height).max(Pixels::ZERO);
        let scroll_top = scroll_top.max(Pixels::ZERO).min(maximum);
        let row = ((scroll_top / viewport.line_height).floor() as usize)
            .min(viewport.line_count.saturating_sub(1));
        self.set_anchor_row(DisplayRow::new(row), snapshot);
        self.anchor.offset.y = scroll_top - viewport.line_height * row;
    }

    /// 把视口顶部锚定到指定显示行：更新派生显示点，并用快照把该行起点重新锚定为组合锚点。
    fn set_anchor_row(&mut self, row: DisplayRow, snapshot: &DisplaySnapshot) {
        let display_point = DisplayPoint::new(row, DisplayColumn::ZERO);
        self.display_point = display_point;
        let offset = snapshot
            .display_point_to_offset(display_point)
            .unwrap_or(zcv_multi_buffer::MultiBufferOffset::ZERO);
        self.anchor.anchor = snapshot
            .buffer_snapshot()
            .anchor_at(offset, Affinity::Before);
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
            anchor: ScrollAnchor::new(),
            display_point: DisplayPoint::ZERO,
            viewport: None,
            pending_autoscroll: None,
            pending_horizontal_autoscroll: false,
            thumb_state: ScrollbarThumbState::Idle,
        }
    }
}

/// 把组合锚点按当前快照解析为显示点。
fn resolve_display_point(
    snapshot: &DisplaySnapshot,
    anchor: &MultiBufferAnchor,
) -> Option<DisplayPoint> {
    let offset = snapshot.buffer_snapshot().resolve_anchor(anchor)?;
    snapshot.offset_to_display_point(offset).ok()
}
