//! 本地光标移动动画：单矩形 + Zed 式阻尼弹簧。
//!
//! 状态由 `Editor` 持有，目标几何与视口由当前帧的 `EditorElement` 提供。
//! 布局变化直接吸附，滚动只平移不重启动画，短移动使用短时长，重定向保留弹簧速度，因此连续输入不会累积滞后。

use std::time::{Duration, Instant};

use gpui::{Bounds, Pixels, Point, point, px, size};
use zcv_multi_buffer::MultiBufferOffset;

// 动力学常量对齐 Zed cursor_animation.rs。
const MAX_FRAME_DURATION: Duration = Duration::from_millis(33);
const SPRING_RESET_EPSILON: f32 = 0.001;
const RECT_ACTIVE_DISTANCE: f32 = 0.5;
const GEOMETRY_EPSILON: f32 = 0.01;
const SHORT_MOVE_THRESHOLD: f32 = 8.0;
const ANIMATION_RESET_THRESHOLD_SECONDS: f32 = 0.075;
const ANIMATION_LENGTH_SECONDS: f32 = 0.125;
const SHORT_ANIMATION_LENGTH_SECONDS: f32 = 0.05;
const MAX_TRAIL_DISTANCE_FACTOR: f32 = 100.0;

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct CursorViewport {
    pub(crate) text_bounds: Bounds<Pixels>,
    pub(crate) scroll: Point<Pixels>,
    pub(crate) line_height: Pixels,
    pub(crate) em_advance: Pixels,
}

impl CursorViewport {
    fn is_finite(self) -> bool {
        [
            f32::from(self.text_bounds.origin.x),
            f32::from(self.text_bounds.origin.y),
            f32::from(self.text_bounds.size.width),
            f32::from(self.text_bounds.size.height),
            f32::from(self.scroll.x),
            f32::from(self.scroll.y),
            f32::from(self.line_height),
            f32::from(self.em_advance),
        ]
        .into_iter()
        .all(f32::is_finite)
    }

    /// 布局是否变化：变化必须吸附，滚动变化只平移。
    fn has_same_layout(self, other: Self) -> bool {
        self.text_bounds == other.text_bounds
            && self.line_height == other.line_height
            && self.em_advance == other.em_advance
    }
}

/// 临界阻尼弹簧的位移与速度，公式与 Zed `DampedSpringAnimation` 相同。
#[derive(Clone, Copy, Default)]
struct DampedSpring {
    position: f32,
    velocity: f32,
}

impl DampedSpring {
    /// 推进一帧；非法输入、超出时长或收敛后清零。
    fn update(&mut self, elapsed_seconds: f32, animation_length: f32) {
        if !elapsed_seconds.is_finite()
            || elapsed_seconds < 0.0
            || !animation_length.is_finite()
            || animation_length <= elapsed_seconds
            || self.position.abs() < SPRING_RESET_EPSILON
        {
            self.reset();
            return;
        }

        // elapsed 为 0 时不推进：跳过浮点运算，保证重定向帧逐位保留速度。
        if elapsed_seconds == 0.0 {
            return;
        }

        let angular_frequency = 4.0 / animation_length;
        let initial_position = self.position;
        let combined_velocity = self.position * angular_frequency + self.velocity;
        let decay = (-angular_frequency * elapsed_seconds).exp();
        self.position = (initial_position + combined_velocity * elapsed_seconds) * decay;
        self.velocity = decay
            * (-initial_position * angular_frequency
                - combined_velocity * elapsed_seconds * angular_frequency
                + combined_velocity);

        if !self.position.is_finite()
            || !self.velocity.is_finite()
            || self.position.abs() < SPRING_RESET_EPSILON
        {
            self.reset();
        }
    }

    fn reset(&mut self) {
        self.position = 0.0;
        self.velocity = 0.0;
    }
}

#[derive(Default)]
pub(crate) struct CursorAnimation {
    /// 当前帧权威目标几何。
    target: Option<Bounds<Pixels>>,
    /// 最近一次推进后的绘制几何。
    current: Option<Bounds<Pixels>>,
    /// 逻辑位置，用于区分「光标移动」与「纯几何变化」。
    offset: Option<MultiBufferOffset>,
    viewport: Option<CursorViewport>,
    /// 位移弹簧，顺序为 x / y / width / height。
    springs: [DampedSpring; 4],
    animation_length: f32,
    last_frame_at: Option<Instant>,
    active: bool,
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
        if !valid_target(target) || !viewport.is_finite() {
            self.clear();
            return None;
        }

        let (Some(mut previous), Some(previous_viewport)) = (self.target, self.viewport) else {
            self.snap(offset, target, viewport, now);
            return None;
        };
        if previous_viewport != viewport {
            if !previous_viewport.has_same_layout(viewport) {
                self.snap(offset, target, viewport, now);
                return None;
            }
            let delta = point(
                previous_viewport.scroll.x - viewport.scroll.x,
                previous_viewport.scroll.y - viewport.scroll.y,
            );
            if let Some(shifted) = self.translate(delta) {
                previous = shifted;
            }
        }

        let moved = !same_bounds(previous, target);
        let offset_changed = self.offset != Some(offset);
        if moved && !offset_changed {
            self.snap(offset, target, viewport, now);
            return None;
        }
        if moved && offset_changed {
            // 静止起步时 elapsed 为 0：本帧仍绘制旧位置，下一帧才开始推进。
            let elapsed = if self.active {
                self.elapsed_since_last_frame(now)
            } else {
                Duration::ZERO
            };
            self.retarget(target);
            self.advance(elapsed);
            self.last_frame_at = Some(now);
        } else if self.active {
            let elapsed = self.elapsed_since_last_frame(now);
            self.advance(elapsed);
            self.last_frame_at = Some(now);
        }
        self.offset = Some(offset);
        self.viewport = Some(viewport);

        if self.active { self.current } else { None }
    }

    /// 滚动平移：目标与当前几何同时位移，弹簧位移不变，动画不重启。
    fn translate(&mut self, delta: Point<Pixels>) -> Option<Bounds<Pixels>> {
        let target = self.target?;
        let current = self.current?;
        let shifted_target = Bounds::new(target.origin + delta, target.size);
        self.target = Some(shifted_target);
        self.current = Some(Bounds::new(current.origin + delta, current.size));
        Some(shifted_target)
    }

    fn retarget(&mut self, target: Bounds<Pixels>) {
        let previous = self.target.expect("retarget 需要前一帧目标几何");
        let current = self.current.unwrap_or(previous);

        let geometry_width = f32::from(target.size.width).max(f32::EPSILON);
        let geometry_height = f32::from(target.size.height).max(f32::EPSILON);
        let horizontal_jump = f32::from(target.origin.x - previous.origin.x) / geometry_width;
        let vertical_jump = f32::from(target.origin.y - previous.origin.y) / geometry_height;
        let is_short_move = horizontal_jump.abs() <= SHORT_MOVE_THRESHOLD
            && vertical_jump.abs() <= SPRING_RESET_EPSILON;
        self.animation_length = if is_short_move {
            ANIMATION_LENGTH_SECONDS.min(SHORT_ANIMATION_LENGTH_SECONDS)
        } else {
            ANIMATION_LENGTH_SECONDS
        };
        // 短移动不重置弹簧，速度跨按键保留；长跳变清零速度。
        if self.animation_length > ANIMATION_RESET_THRESHOLD_SECONDS {
            for spring in &mut self.springs {
                spring.reset();
            }
        }

        let destinations = [
            f32::from(target.origin.x),
            f32::from(target.origin.y),
            f32::from(target.size.width),
            f32::from(target.size.height),
        ];
        let origins = [
            f32::from(current.origin.x),
            f32::from(current.origin.y),
            f32::from(current.size.width),
            f32::from(current.size.height),
        ];
        for (spring, (destination, origin)) in self
            .springs
            .iter_mut()
            .zip(destinations.into_iter().zip(origins))
        {
            spring.position = destination - origin;
        }
        self.target = Some(target);
        self.active = self.springs.iter().any(|spring| spring.position != 0.0);
    }

    fn advance(&mut self, elapsed: Duration) {
        let elapsed_seconds = elapsed.as_secs_f32();
        let max_trail_distance = self
            .target
            .map(|target| {
                f32::from(target.size.width.max(target.size.height)) * MAX_TRAIL_DISTANCE_FACTOR
            })
            .unwrap_or_default();
        let animation_length = self.animation_length;
        let mut active = false;
        for spring in &mut self.springs {
            spring.update(elapsed_seconds, animation_length);
            spring.position = spring
                .position
                .clamp(-max_trail_distance, max_trail_distance);
            active |= spring.position.abs() > RECT_ACTIVE_DISTANCE;
        }
        self.active = active;

        let target = self.target.expect("advance 需要目标几何");
        if active {
            let x = f32::from(target.origin.x) - self.springs[0].position;
            let y = f32::from(target.origin.y) - self.springs[1].position;
            let width = (f32::from(target.size.width) - self.springs[2].position).max(0.0);
            let height = (f32::from(target.size.height) - self.springs[3].position).max(0.0);
            self.current = Some(Bounds::new(
                point(px(x), px(y)),
                size(px(width), px(height)),
            ));
        } else {
            for spring in &mut self.springs {
                spring.reset();
            }
            self.current = Some(target);
        }
    }

    fn snap(
        &mut self,
        offset: MultiBufferOffset,
        target: Bounds<Pixels>,
        viewport: CursorViewport,
        now: Instant,
    ) {
        self.target = Some(target);
        self.current = Some(target);
        self.offset = Some(offset);
        self.viewport = Some(viewport);
        self.last_frame_at = Some(now);
        self.active = false;
        for spring in &mut self.springs {
            spring.reset();
        }
    }

    fn elapsed_since_last_frame(&self, now: Instant) -> Duration {
        self.last_frame_at
            .and_then(|last_frame_at| now.checked_duration_since(last_frame_at))
            .unwrap_or_default()
            .min(MAX_FRAME_DURATION)
    }
}

fn valid_target(target: Bounds<Pixels>) -> bool {
    [
        f32::from(target.left()),
        f32::from(target.top()),
        f32::from(target.size.width),
        f32::from(target.size.height),
    ]
    .into_iter()
    .all(f32::is_finite)
        && target.size.width > Pixels::ZERO
        && target.size.height > Pixels::ZERO
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
