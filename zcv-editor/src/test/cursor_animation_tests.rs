use gpui::{Bounds, point, px, size};

use super::*;

fn viewport(scroll_y: f32) -> CursorViewport {
    CursorViewport {
        text_bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(800.0), px(600.0))),
        scroll: point(px(0.0), px(scroll_y)),
        line_height: px(20.0),
        em_advance: px(8.0),
    }
}

fn bounds(x: f32, y: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(2.0), px(20.0)))
}

fn position_at(bounds: Bounds<Pixels>) -> (f32, f32) {
    (f32::from(bounds.left()), f32::from(bounds.top()))
}

#[test]
fn first_position_snaps_without_animation() {
    let now = Instant::now();
    let mut animation = CursorAnimation::default();
    assert!(
        animation
            .update(
                MultiBufferOffset::new(0),
                bounds(0.0, 0.0),
                viewport(0.0),
                now
            )
            .is_none()
    );
}

#[test]
fn one_cell_move_uses_short_length_and_starts_from_previous_position() {
    let now = Instant::now();
    let mut animation = CursorAnimation::default();
    animation.update(
        MultiBufferOffset::new(0),
        bounds(0.0, 0.0),
        viewport(0.0),
        now,
    );

    let start = animation
        .update(
            MultiBufferOffset::new(10),
            bounds(8.0, 0.0),
            viewport(0.0),
            now,
        )
        .expect("移动开始时应绘制旧位置");
    assert_eq!(position_at(start), (0.0, 0.0));
    assert!((animation.animation_length - SHORT_ANIMATION_LENGTH_SECONDS).abs() < f32::EPSILON);

    let middle = animation
        .update(
            MultiBufferOffset::new(10),
            bounds(8.0, 0.0),
            viewport(0.0),
            now + Duration::from_millis(16),
        )
        .expect("过渡中应继续绘制动画光标");
    assert!(f32::from(middle.left()) > 0.0 && f32::from(middle.left()) < 8.0);
}

#[test]
fn vertical_move_uses_long_length() {
    let now = Instant::now();
    let mut animation = CursorAnimation::default();
    animation.update(
        MultiBufferOffset::new(0),
        bounds(0.0, 0.0),
        viewport(0.0),
        now,
    );
    animation.update(
        MultiBufferOffset::new(10),
        bounds(0.0, 20.0),
        viewport(0.0),
        now,
    );
    assert!((animation.animation_length - ANIMATION_LENGTH_SECONDS).abs() < f32::EPSILON);
}

#[test]
fn mid_flight_retarget_preserves_momentum() {
    let now = Instant::now();
    let mut animation = CursorAnimation::default();
    animation.update(
        MultiBufferOffset::new(0),
        bounds(0.0, 0.0),
        viewport(0.0),
        now,
    );
    animation.update(
        MultiBufferOffset::new(1),
        bounds(8.0, 0.0),
        viewport(0.0),
        now,
    );
    let halfway = now + Duration::from_millis(16);
    animation.update(
        MultiBufferOffset::new(1),
        bounds(8.0, 0.0),
        viewport(0.0),
        halfway,
    );
    let velocity = animation.springs[0].velocity;
    assert_ne!(velocity, 0.0, "半途应已有速度");

    // 同一时刻重定向：短移动不重置弹簧，速度必须原样保留。
    animation.update(
        MultiBufferOffset::new(2),
        bounds(16.0, 0.0),
        viewport(0.0),
        halfway,
    );
    assert_eq!(animation.springs[0].velocity, velocity);
}

#[test]
fn long_move_resets_velocity() {
    let now = Instant::now();
    let mut animation = CursorAnimation::default();
    animation.update(
        MultiBufferOffset::new(0),
        bounds(0.0, 0.0),
        viewport(0.0),
        now,
    );
    animation.update(
        MultiBufferOffset::new(1),
        bounds(8.0, 0.0),
        viewport(0.0),
        now,
    );
    for frame in 1..10 {
        animation.update(
            MultiBufferOffset::new(1),
            bounds(8.0, 0.0),
            viewport(0.0),
            now + Duration::from_millis(frame * 16),
        );
    }
    assert!(!animation.active, "短移动应已收敛");

    // 跨行是长跳变：即使静止起步，也应清零上一段速度。
    animation.update(
        MultiBufferOffset::new(2),
        bounds(8.0, 20.0),
        viewport(0.0),
        now + Duration::from_millis(200),
    );
    assert_eq!(animation.springs[0].velocity, 0.0);
    assert_eq!(animation.springs[1].velocity, 0.0);
}

#[test]
fn continuous_short_input_does_not_accumulate_lag() {
    let start = Instant::now();
    let interval = Duration::from_millis(30);
    let frame = Duration::from_micros(16_667);
    let mut animation = CursorAnimation::default();
    animation.update(
        MultiBufferOffset::new(0),
        bounds(0.0, 0.0),
        viewport(0.0),
        start,
    );

    let mut offset = 0usize;
    let mut next_key = start + interval;
    let mut time = start;
    let mut max_lag = 0.0f32;
    let end = start + interval * 20;
    while time <= end {
        if time >= next_key && offset < 20 {
            offset += 1;
            next_key += interval;
        }
        let target_x = offset as f32 * 8.0;
        animation.update(
            MultiBufferOffset::new(offset),
            bounds(target_x, 0.0),
            viewport(0.0),
            time,
        );
        let drawn_x = f32::from(animation.current.expect("动画始终有当前几何").left());
        max_lag = max_lag.max((target_x - drawn_x).max(0.0));
        time += frame;
    }

    // 连续输入期间滞后不得超过一个格宽，且必须能收敛回目标；不会随按键累积。
    assert!(max_lag <= 8.0 + 0.01, "峰值滞后应不超过一格：{max_lag}");
    let mut settle = time;
    for _ in 0..20 {
        settle += frame;
        animation.update(
            MultiBufferOffset::new(offset),
            bounds(offset as f32 * 8.0, 0.0),
            viewport(0.0),
            settle,
        );
    }
    assert!(!animation.active, "停止输入后应收敛");
    assert_eq!(
        f32::from(animation.current.expect("收敛后仍有几何").left()),
        offset as f32 * 8.0
    );
}

#[test]
fn scrolling_translates_active_cursor_without_restarting_animation() {
    let now = Instant::now();
    let mut animation = CursorAnimation::default();
    animation.update(
        MultiBufferOffset::new(0),
        bounds(0.0, 0.0),
        viewport(0.0),
        now,
    );
    animation.update(
        MultiBufferOffset::new(1),
        bounds(8.0, 0.0),
        viewport(0.0),
        now,
    );
    let before = animation
        .update(
            MultiBufferOffset::new(1),
            bounds(8.0, 0.0),
            viewport(0.0),
            now + Duration::from_millis(16),
        )
        .unwrap();

    let after = animation
        .update(
            MultiBufferOffset::new(1),
            bounds(8.0, -20.0),
            viewport(20.0),
            now + Duration::from_millis(16),
        )
        .unwrap();
    assert_eq!(after.left(), before.left());
    assert_eq!(after.top(), before.top() - px(20.0));
}

#[test]
fn geometry_change_without_cursor_movement_snaps() {
    let now = Instant::now();
    let mut animation = CursorAnimation::default();
    animation.update(
        MultiBufferOffset::new(0),
        bounds(0.0, 0.0),
        viewport(0.0),
        now,
    );
    assert!(
        animation
            .update(
                MultiBufferOffset::new(0),
                bounds(10.0, 0.0),
                viewport(0.0),
                now
            )
            .is_none()
    );
    assert!(!animation.active);
}

#[test]
fn layout_change_snaps_even_when_animation_is_active() {
    let now = Instant::now();
    let mut animation = CursorAnimation::default();
    animation.update(
        MultiBufferOffset::new(0),
        bounds(0.0, 0.0),
        viewport(0.0),
        now,
    );
    animation.update(
        MultiBufferOffset::new(1),
        bounds(8.0, 0.0),
        viewport(0.0),
        now,
    );
    let resized = CursorViewport {
        text_bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(700.0), px(600.0))),
        ..viewport(0.0)
    };
    assert!(
        animation
            .update(
                MultiBufferOffset::new(1),
                bounds(8.0, 0.0),
                resized,
                now + Duration::from_millis(16)
            )
            .is_none()
    );
    assert!(!animation.active);
}

#[test]
fn invalid_geometry_clears_animation() {
    let now = Instant::now();
    let mut animation = CursorAnimation::default();
    animation.update(
        MultiBufferOffset::new(0),
        bounds(0.0, 0.0),
        viewport(0.0),
        now,
    );
    let zero_width = Bounds::new(point(px(0.0), px(0.0)), size(px(0.0), px(20.0)));
    assert!(
        animation
            .update(MultiBufferOffset::new(0), zero_width, viewport(0.0), now)
            .is_none()
    );
    assert!(animation.target.is_none() && animation.current.is_none());
}

#[test]
fn clear_forgets_all_state() {
    let now = Instant::now();
    let mut animation = CursorAnimation::default();
    animation.update(
        MultiBufferOffset::new(0),
        bounds(0.0, 0.0),
        viewport(0.0),
        now,
    );
    animation.update(
        MultiBufferOffset::new(1),
        bounds(8.0, 0.0),
        viewport(0.0),
        now,
    );
    animation.clear();
    assert!(animation.target.is_none());
    assert!(animation.current.is_none());
    assert!(!animation.active);
}
