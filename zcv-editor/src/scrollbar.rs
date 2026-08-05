//! 垂直滚动轴：track/thumb 几何与逐帧布局数据。
//!
//! 事件处理与绘制在 EditorElement 中完成（对齐 gutter.rs 的职责划分）；
//! 本模块只负责几何计算，全部公式用 Pixels 模型表达。

use gpui::{Bounds, Hitbox, HitboxBehavior, Pixels, Point, Window, point, px, size};

use super::scroll::ScrollbarThumbState;

/// 滚动轴宽度，与 Zed 的 `ui::EDITOR_SCROLLBAR_WIDTH` 一致。
pub(super) const SCROLLBAR_WIDTH: Pixels = px(15.);

/// thumb 最小高度，与 Zed 的 `ScrollbarLayout::MIN_THUMB_SIZE` 一致。
pub(super) const MIN_THUMB_SIZE: Pixels = px(25.);

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

#[cfg(test)]
mod tests {
    use super::*;

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
