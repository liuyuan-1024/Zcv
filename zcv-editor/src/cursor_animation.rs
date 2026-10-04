//! 本地光标移动动画。状态由 Editor 持有，目标几何由当前帧的 EditorElement 提供。

use std::time::{Duration, Instant};

use gpui::{Bounds, Pixels, Point, point, px, size};
use zcv_multi_buffer::MultiBufferOffset;

// VS Code 的平滑光标移动使用 80ms 的 CSS 过渡。
const TRANSITION_DURATION: Duration = Duration::from_millis(80);
const GEOMETRY_EPSILON: f32 = 0.01;

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct CursorViewport {
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) scroll: Point<Pixels>,
}

#[derive(Clone, Copy)]
struct Transition {
    from: Bounds<Pixels>,
    to: Bounds<Pixels>,
    started_at: Instant,
}

impl Transition {
    fn sample(self, now: Instant) -> Option<Bounds<Pixels>> {
        let elapsed = now
            .checked_duration_since(self.started_at)
            .unwrap_or_default();
        if elapsed >= TRANSITION_DURATION {
            return None;
        }
        let progress = css_ease(elapsed.as_secs_f32() / TRANSITION_DURATION.as_secs_f32());
        Some(Bounds::new(
            point(
                px(lerp(self.from.origin.x, self.to.origin.x, progress)),
                px(lerp(self.from.origin.y, self.to.origin.y, progress)),
            ),
            size(
                px(lerp(self.from.size.width, self.to.size.width, progress)),
                px(lerp(self.from.size.height, self.to.size.height, progress)),
            ),
        ))
    }

    fn translate(&mut self, delta: Point<Pixels>) {
        self.from.origin.x += delta.x;
        self.from.origin.y += delta.y;
        self.to.origin.x += delta.x;
        self.to.origin.y += delta.y;
    }
}

fn lerp(from: Pixels, to: Pixels, progress: f32) -> f32 {
    f32::from(from) + f32::from(to - from) * progress
}

fn bezier_component(parameter: f32, first: f32, second: f32) -> f32 {
    let inverse = 1.0 - parameter;
    3.0 * inverse * inverse * parameter * first
        + 3.0 * inverse * parameter * parameter * second
        + parameter * parameter * parameter
}

/// CSS 默认 ease 曲线：cubic-bezier(0.25, 0.1, 0.25, 1)。
fn css_ease(progress: f32) -> f32 {
    if progress <= 0.0 {
        return 0.0;
    }
    if progress >= 1.0 {
        return 1.0;
    }
    let mut lower = 0.0;
    let mut upper = 1.0;
    for _ in 0..16 {
        let parameter = (lower + upper) / 2.0;
        if bezier_component(parameter, 0.25, 0.25) < progress {
            lower = parameter;
        } else {
            upper = parameter;
        }
    }
    bezier_component((lower + upper) / 2.0, 0.1, 1.0)
}

#[derive(Default)]
pub(crate) struct CursorAnimation {
    target: Option<Bounds<Pixels>>,
    offset: Option<MultiBufferOffset>,
    viewport: Option<CursorViewport>,
    transition: Option<Transition>,
}

impl CursorAnimation {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn update(
        &mut self,
        offset: MultiBufferOffset,
        target: Bounds<Pixels>,
        viewport: CursorViewport,
        now: Instant,
    ) -> Option<Bounds<Pixels>> {
        let valid = [
            f32::from(target.left()),
            f32::from(target.top()),
            f32::from(target.size.width),
            f32::from(target.size.height),
            f32::from(viewport.scroll.x),
            f32::from(viewport.scroll.y),
            f32::from(viewport.bounds.left()),
            f32::from(viewport.bounds.top()),
            f32::from(viewport.bounds.size.width),
            f32::from(viewport.bounds.size.height),
        ]
        .into_iter()
        .all(f32::is_finite)
            && target.size.width > Pixels::ZERO
            && target.size.height > Pixels::ZERO;
        if !valid {
            self.clear();
            return None;
        }
        let (Some(mut previous), Some(previous_viewport)) = (self.target, self.viewport) else {
            self.snap(offset, target, viewport);
            return None;
        };
        if previous_viewport.bounds != viewport.bounds {
            self.snap(offset, target, viewport);
            return None;
        }
        if previous_viewport.scroll != viewport.scroll {
            let delta = point(
                previous_viewport.scroll.x - viewport.scroll.x,
                previous_viewport.scroll.y - viewport.scroll.y,
            );
            previous.origin.x += delta.x;
            previous.origin.y += delta.y;
            if let Some(transition) = &mut self.transition {
                transition.translate(delta);
            }
        }

        let moved = !same_bounds(previous, target);
        let offset_changed = self.offset != Some(offset);
        if moved && !offset_changed {
            self.snap(offset, target, viewport);
            return None;
        }
        if moved && offset_changed {
            let from = self
                .transition
                .and_then(|transition| transition.sample(now))
                .unwrap_or(previous);
            self.transition = Some(Transition {
                from,
                to: target,
                started_at: now,
            });
        }
        self.target = Some(target);
        self.offset = Some(offset);
        self.viewport = Some(viewport);

        let current = self
            .transition
            .and_then(|transition| transition.sample(now));
        if current.is_none() {
            self.transition = None;
        }
        current
    }

    fn snap(
        &mut self,
        offset: MultiBufferOffset,
        target: Bounds<Pixels>,
        viewport: CursorViewport,
    ) {
        self.target = Some(target);
        self.offset = Some(offset);
        self.viewport = Some(viewport);
        self.transition = None;
    }
}

fn same_bounds(left: Bounds<Pixels>, right: Bounds<Pixels>) -> bool {
    [
        f32::from(left.left() - right.left()),
        f32::from(left.top() - right.top()),
        f32::from(left.size.width - right.size.width),
        f32::from(left.size.height - right.size.height),
    ]
    .into_iter()
    .all(|delta| delta.abs() <= GEOMETRY_EPSILON)
}

#[cfg(test)]
#[path = "test/cursor_animation_tests.rs"]
mod tests;
