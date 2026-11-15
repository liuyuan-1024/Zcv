//! 共用圆环轨道的进度指示器。

use std::{f32::consts::TAU, time::Duration};

use gpui::{
    Animation, AnimationExt, App, ElementId, Hsla, IntoElement, Path, PathBuilder, Pixels, Point,
    RenderOnce, Window, canvas, div, point, prelude::*, px,
};
use zcv_theme::{color, scale};

const ROTATION_DURATION: Duration = Duration::from_secs(1);
const ROTATING_ARC_FRACTION: f32 = 0.25;

/// 圆环的进度语义。
#[derive(Clone, Copy, Debug)]
pub enum ProgressMode {
    /// 从圆顶顺时针显示给定比例，调用方负责更新比例。
    Determinate(f32),
    /// 持续旋转的短弧，用于没有明确比例的活动。
    Indeterminate,
}

/// 显示已知比例或持续活动的圆环。
#[derive(IntoElement)]
pub struct ProgressIndicator {
    id: ElementId,
    mode: ProgressMode,
}

impl ProgressIndicator {
    /// `id` 在所在作用域内应稳定且唯一，用于接续旋转动画。
    pub fn new(id: impl Into<ElementId>, mode: ProgressMode) -> Self {
        Self {
            id: id.into(),
            mode,
        }
    }
}

impl RenderOnce for ProgressIndicator {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let size = scale::to_pixels(scale::S12, window);
        let colors = color::current(cx);
        let track = colors.border.into();
        let foreground = colors.text_muted.into();

        match self.mode {
            ProgressMode::Determinate(fraction) => {
                ring(size, 0.0, fraction.clamp(0.0, 1.0), track, foreground).into_any_element()
            }
            ProgressMode::Indeterminate => div()
                .flex_shrink_0()
                .with_animation(
                    self.id,
                    Animation::new(ROTATION_DURATION).repeat(),
                    move |container, phase| {
                        container.child(ring(size, phase, ROTATING_ARC_FRACTION, track, foreground))
                    },
                )
                .into_any_element(),
        }
    }
}

fn ring(
    size: Pixels,
    start: f32,
    fraction: f32,
    track: Hsla,
    foreground: Hsla,
) -> impl IntoElement {
    let stroke_width = size / 8.0;
    let radius = (size - stroke_width) / 2.0;
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let center = bounds.center();
            if let Some(path) = arc_path(center, radius, stroke_width, 0.0, 1.0) {
                window.paint_path(path, track);
            }
            if fraction > 0.0
                && let Some(path) = arc_path(center, radius, stroke_width, start, fraction)
            {
                window.paint_path(path, foreground);
            }
        },
    )
    .size(size)
    .flex_shrink_0()
}

fn arc_path(
    center: Point<Pixels>,
    radius: Pixels,
    stroke_width: Pixels,
    start: f32,
    fraction: f32,
) -> Option<Path<Pixels>> {
    let point_at = |turns: f32| {
        let (sin, cos) = (turns * TAU).sin_cos();
        center + point(radius * sin, -radius * cos)
    };
    let mut builder = PathBuilder::stroke(stroke_width);
    let radii = point(radius, radius);
    let first = point_at(start);
    builder.move_to(first);
    if fraction == 1.0 {
        // 完整圆周需要两段弧，首尾重合的单段弧不会形成圆。
        builder.arc_to(radii, px(0.0), false, true, point_at(start + 0.5));
        builder.arc_to(radii, px(0.0), false, true, first);
        builder.close();
    } else {
        builder.arc_to(
            radii,
            px(0.0),
            fraction > 0.5,
            true,
            point_at(start + fraction),
        );
    }
    builder.build().ok()
}

#[cfg(test)]
#[path = "test/progress_indicator_tests.rs"]
mod tests;
