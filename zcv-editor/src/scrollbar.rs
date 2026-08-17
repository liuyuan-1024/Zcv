//! 垂直滚动轴：track/thumb 几何与逐帧布局数据。
//!
//! 事件处理与绘制在 EditorElement 中完成（对齐 gutter.rs 的职责划分）；
//! 本模块只负责几何计算，全部公式用 Pixels 模型表达。

use std::ops::Range;

use gpui::{Bounds, Hitbox, HitboxBehavior, Pixels, Point, Window, point, px, size};
use zcv_buffer_diff::DiffHunkKind;

use super::scroll::ScrollbarThumbState;

/// 滚动轴宽度，与 Zed 的 `ui::EDITOR_SCROLLBAR_WIDTH` 一致。
pub(super) const SCROLLBAR_WIDTH: Pixels = px(15.);

/// thumb 最小高度，与 Zed 的 `ScrollbarLayout::MIN_THUMB_SIZE` 一致。
pub(super) const MIN_THUMB_SIZE: Pixels = px(25.);

/// 轨道宽 − 边框后等分的列数（对齐 Zed 的三列 marker 坐标系；第 0 列显示 git diff）。
const MARKER_COLUMN_COUNT: f32 = 3.0;

/// 轨道左右边框宽度（对齐 Zed `ScrollbarLayout::BORDER_WIDTH`）。
const SCROLLBAR_BORDER: Pixels = px(1.);

/// marker 最小高度（对齐 Zed `MIN_MARKER_HEIGHT`：单行 hunk 至少 5px 可见）。
const MIN_MARKER_HEIGHT: Pixels = px(5.);

/// 垂直滚动轴的逐帧布局：track hitbox、thumb 几何与三态。
#[derive(Clone)]
pub(super) struct ScrollbarLayout {
    /// track 命中区域（同时作为绘制边界）。
    pub(super) hitbox: Hitbox,
    /// thumb 边界；内容不高于视口时为 None。
    pub(super) thumb_bounds: Option<Bounds<Pixels>>,
    /// 内容像素 / 轨道像素换算系数 = max_scroll / (track_length − thumb_size)。
    /// 拖动增量与点击跳页共用（与 Zed 的 text_unit_size 数学等价）。
    pub(super) scroll_per_pixel: f32,
    /// 当前三态（每帧由 ScrollManager 的跨帧状态填充，决定绘制颜色）。
    pub(super) thumb_state: ScrollbarThumbState,
    /// 本帧的 git diff marker（prepaint 计算，paint 只画）。
    pub(super) markers: Vec<ScrollbarMarker>,
}

/// 滚动轴上单个 diff marker：轨道内 y 区间 + 颜色类别。
#[derive(Clone, Debug)]
pub(super) struct ScrollbarMarker {
    pub(super) y_range: Range<Pixels>,
    pub(super) kind: DiffHunkKind,
}

impl ScrollbarLayout {
    /// 由 track 边界与滚动状态构造布局。Full 模式下 track hitbox 恒创建。
    pub(super) fn new(
        track_bounds: Bounds<Pixels>,
        max_scroll: Pixels,
        scroll_top: Pixels,
        thumb_state: ScrollbarThumbState,
        window: &mut Window,
    ) -> Self {
        let hitbox = window.insert_hitbox(track_bounds, HitboxBehavior::Normal);
        let (thumb_bounds, scroll_per_pixel) = thumb_geometry(track_bounds, max_scroll, scroll_top)
            .map_or((None, 0.0), |(bounds, scale)| (Some(bounds), scale));
        Self {
            hitbox,
            thumb_bounds,
            scroll_per_pixel,
            thumb_state,
            markers: Vec::new(),
        }
    }

    /// 指针是否落在 thumb 上（悬停与释放判定）。
    pub(super) fn thumb_hovered(&self, position: &Point<Pixels>) -> bool {
        self.thumb_bounds
            .is_some_and(|bounds| bounds.contains(position))
    }
}

/// 纯几何：由轨道边界、内容高度与滚动位置推导 thumb 与换算系数。
///
/// 数学（与 Zed 的 ScrollbarLayout::new 等价）：
/// - total = max_scroll + track_length（内容总高，轨道高度即视口高度）
/// - thumb_size = track_length × track_length/total，夹 [MIN_THUMB_SIZE, track_length]
/// - travel = track_length − thumb_size（thumb 可活动行程），下限 1px 防除零
/// - scroll_per_pixel = max_scroll / travel
/// - thumb 顶 = scroll_top / scroll_per_pixel（scroll_top=0 贴顶、=max_scroll 贴底）
///
/// 内容不高于视口（max_scroll ≤ 0）时返回 None。
pub(super) fn thumb_geometry(
    track_bounds: Bounds<Pixels>,
    max_scroll: Pixels,
    scroll_top: Pixels,
) -> Option<(Bounds<Pixels>, f32)> {
    if max_scroll <= Pixels::ZERO {
        return None;
    }
    let track_length = track_bounds.size.height;
    let total = max_scroll + track_length;
    let thumb_size = (track_length * (track_length / total))
        .max(MIN_THUMB_SIZE)
        .min(track_length);
    // 轨道不足 25px 时 thumb 占满轨道，travel 下限 1px 保证换算系数有界。
    let travel = (track_length - thumb_size).max(px(1.));
    let scroll_per_pixel = max_scroll / travel;
    let thumb_top = scroll_top * (1.0 / scroll_per_pixel);
    let thumb_bounds = Bounds {
        origin: point(track_bounds.origin.x, track_bounds.origin.y + thumb_top),
        size: size(track_bounds.size.width, thumb_size),
    };
    Some((thumb_bounds, scroll_per_pixel))
}

/// 纯几何：显示行区间列表 → 轨道内 marker 区间列表。
///
/// 公式（对齐 Zed `marker_quads_for_ranges`）：
/// - content_y = row × line_height；track_y = track.top + content_y / scroll_per_pixel
/// - **绝对定位**：marker 表示行在文档中的位置（与 thumb 同一坐标系，不随滚动变化），拖动 thumb 到 marker 处即精确滚动到该行
/// - scroll_per_pixel = 0（内容不溢出视口）时 content_y 即轨道坐标（轨道高 == 视口高）
/// - 高度夹到 [MIN_MARKER_HEIGHT, ∞)；相邻同色区间间隙 ≤ 1px 时合并
/// - 与轨道 y 区间不相交的 marker 丢弃（只画可见段）
pub(super) fn marker_geometry(
    row_ranges: impl IntoIterator<Item = (Range<usize>, DiffHunkKind)>,
    track_bounds: Bounds<Pixels>,
    scroll_per_pixel: f32,
    line_height: Pixels,
) -> Vec<ScrollbarMarker> {
    let to_track_y = |content_y: Pixels| -> Pixels {
        if scroll_per_pixel > 0.0 {
            track_bounds.top() + content_y * (1.0 / scroll_per_pixel)
        } else {
            track_bounds.top() + content_y
        }
    };
    let row_to_px = |row: usize| line_height * row;
    let markers: Vec<ScrollbarMarker> = row_ranges
        .into_iter()
        .filter_map(|(range, kind)| {
            let start_y = to_track_y(row_to_px(range.start));
            let mut end_y = to_track_y(row_to_px(range.end));
            // 单行（或 0 宽）hunk 至少 MIN_MARKER_HEIGHT 高，保证可见。
            if end_y - start_y < MIN_MARKER_HEIGHT {
                end_y = start_y + MIN_MARKER_HEIGHT;
            }
            (end_y > track_bounds.top() && start_y < track_bounds.bottom()).then_some(
                ScrollbarMarker {
                    y_range: start_y..end_y,
                    kind,
                },
            )
        })
        .collect();
    // 相邻同色且间隙 ≤ 1px 的 marker 合并成一段（对齐 Zed 的合并策略）。
    let mut merged: Vec<ScrollbarMarker> = Vec::new();
    for marker in markers {
        if let Some(last) = merged.last_mut()
            && last.kind == marker.kind
            && marker.y_range.start <= last.y_range.end + px(1.)
        {
            last.y_range.end = last.y_range.end.max(marker.y_range.end);
        } else {
            merged.push(marker);
        }
    }
    merged
}

/// 第 0 列（git diff）的横向区间：轨道左边 + 1px 边框起，宽一列（Zed 同式）。
pub(super) fn marker_column_x_range(track_bounds: Bounds<Pixels>) -> Range<Pixels> {
    let width = (track_bounds.size.width - SCROLLBAR_BORDER) * (1.0 / MARKER_COLUMN_COUNT);
    let start = track_bounds.left() + SCROLLBAR_BORDER;
    start..(start + width.floor())
}

#[cfg(test)]
mod tests {
    use super::*;
    use DiffHunkKind::*;

    fn track_bounds(height: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(15.), px(height)),
        }
    }

    #[test]
    fn marker_geometry_maps_rows_to_track_absolutely() {
        // per_pixel=2（内容 200px ↔ 轨道 100px）：行 5（content 125）→ 轨道 125/2=62.5。
        // 绝对定位：marker 表示行在文档中的位置，不随滚动变化（与 thumb 同一坐标系）。
        let track = track_bounds(100.);
        let markers = marker_geometry([(5..6, Modified)], track, 2.0, px(25.));
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].y_range.start, px(62.5));
        assert_eq!(markers[0].y_range.end, px(62.5 + 12.5));

        // 拖动 thumb 到 marker 处：scroll_top = 62.5 × 2 = 125 = 行 5 的 content_y（精确对齐）。
        assert_eq!(px(62.5) * 2.0, px(125.));
    }

    #[test]
    fn marker_geometry_enforces_minimum_height() {
        // 内容高度 == 视口高度（per_pixel=0）：行 0 的 5px 高 marker 直接映射。
        let track = track_bounds(200.);
        let markers = marker_geometry([(0..1, Added)], track, 0.0, px(25.));
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].y_range.start, px(0.));
        // 行 0 的 content 区间 [0, 25)，不足 5px？不——25px > 5px，无需夹取。
        assert_eq!(markers[0].y_range.end, px(25.));

        // 极端缩放（内容远大于视口）下单行被压到 < 5px → 夹取到 5px。
        let track = track_bounds(200.);
        let markers = marker_geometry([(0..1, Added)], track, 40.0, px(25.));
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].y_range.end - markers[0].y_range.start, px(5.));
    }

    #[test]
    fn marker_geometry_merges_adjacent_same_kind_and_keeps_different_kinds() {
        let track = track_bounds(200.);
        // 同色相邻（间隙 0）合并。
        let markers = marker_geometry([(0..1, Added), (1..2, Added)], track, 0.0, px(25.));
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].y_range, px(0.)..px(50.));

        // 异色不合并。
        let markers = marker_geometry([(0..1, Added), (1..2, Deleted)], track, 0.0, px(25.));
        assert_eq!(markers.len(), 2);
    }

    #[test]
    fn marker_geometry_discards_markers_outside_track() {
        let track = track_bounds(100.);
        // per_pixel=2：行 10（content 250..275 → track 125..137.5）超出视口 → 丢弃。
        let markers = marker_geometry([(10..11, Modified)], track, 2.0, px(25.));
        assert!(markers.is_empty());
    }

    #[test]
    fn marker_column_x_range_divides_track_into_three_columns() {
        let track = track_bounds(200.);
        let x_range = marker_column_x_range(track);
        // (15 − 1) / 3 = 4.67 → floor 4；从 1px 边框起。
        assert_eq!(x_range, px(1.)..px(5.));
    }

    #[test]
    fn thumb_geometry_maps_scroll_range_to_track() {
        let track = Bounds {
            origin: point(px(10.), px(20.)),
            size: size(px(15.), px(200.)),
        };

        let (bounds, per_pixel) = thumb_geometry(track, px(300.), px(0.)).unwrap();
        // total=500，thumb=200×200/500=80，travel=120，per_pixel=300/120=2.5
        assert_eq!(per_pixel, 2.5);
        assert_eq!(bounds.size, size(px(15.), px(80.)));
        assert_eq!(bounds.origin, point(px(10.), px(20.))); // scroll_top=0 贴顶

        let (bounds, per_pixel) = thumb_geometry(track, px(300.), px(300.)).unwrap();
        assert_eq!(per_pixel, 2.5);
        assert_eq!(bounds.origin.y, px(140.)); // scroll_top=max 贴底
    }

    #[test]
    fn thumb_geometry_is_absent_without_overflow() {
        let track = Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(15.), px(200.)),
        };
        assert_eq!(thumb_geometry(track, px(0.), px(0.)), None);
        assert_eq!(thumb_geometry(track, px(-5.), px(0.)), None);
    }

    #[test]
    fn thumb_geometry_clamps_minimum_thumb_size() {
        let track = Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(15.), px(200.)),
        };

        let (bounds, _) = thumb_geometry(track, px(1_900.), px(0.)).unwrap();
        // total=2100，thumb=200×200/2100≈19 < 25 → 夹到 25
        assert_eq!(bounds.size.height, px(25.));

        let (_, per_pixel) = thumb_geometry(track, px(1_900.), px(1_900.)).unwrap();
        assert_eq!(per_pixel, 1_900. / 175.);
    }
}
