use gpui::{Bounds, point, px, size};

use super::*;

fn bounds(x: f32, y: f32) -> Bounds<Pixels> {
    Bounds::new(point(px(x), px(y)), size(px(2.0), px(20.0)))
}

fn viewport(scroll_y: f32) -> CursorViewport {
    CursorViewport {
        bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(800.0), px(600.0))),
        scroll: point(px(0.0), px(scroll_y)),
    }
}

fn position_at(bounds: Bounds<Pixels>) -> (f32, f32) {
    (f32::from(bounds.left()), f32::from(bounds.top()))
}

#[test]
fn first_position_snaps_and_movement_uses_eighty_millisecond_ease() {
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

    let start = animation
        .update(
            MultiBufferOffset::new(10),
            bounds(100.0, 0.0),
            viewport(0.0),
            now,
        )
        .expect("移动开始时应绘制旧位置");
    assert_eq!(position_at(start), (0.0, 0.0));
    let middle = animation
        .update(
            MultiBufferOffset::new(10),
            bounds(100.0, 0.0),
            viewport(0.0),
            now + Duration::from_millis(40),
        )
        .expect("过渡中应继续绘制动画光标");
    let (x, y) = position_at(middle);
    assert!((79.0..=81.0).contains(&x));
    assert_eq!(y, 0.0);
    assert_eq!(middle.size, start.size);
    assert!(
        animation
            .update(
                MultiBufferOffset::new(10),
                bounds(100.0, 0.0),
                viewport(0.0),
                now + Duration::from_millis(80),
            )
            .is_none()
    );
}

#[test]
fn cursor_size_changes_with_the_same_transition() {
    let now = Instant::now();
    let mut animation = CursorAnimation::default();
    animation.update(
        MultiBufferOffset::new(0),
        bounds(0.0, 0.0),
        viewport(0.0),
        now,
    );
    let larger = Bounds::new(point(px(100.0), px(40.0)), size(px(10.0), px(30.0)));
    animation.update(MultiBufferOffset::new(10), larger, viewport(0.0), now);

    let middle = animation
        .update(
            MultiBufferOffset::new(10),
            larger,
            viewport(0.0),
            now + Duration::from_millis(40),
        )
        .unwrap();
    assert!(middle.size.width > px(2.0) && middle.size.width < px(10.0));
    assert!(middle.size.height > px(20.0) && middle.size.height < px(30.0));
}

#[test]
fn movement_retargets_from_current_animated_position() {
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
        bounds(100.0, 0.0),
        viewport(0.0),
        now,
    );
    let halfway = now + Duration::from_millis(40);
    let current = animation
        .update(
            MultiBufferOffset::new(10),
            bounds(100.0, 0.0),
            viewport(0.0),
            halfway,
        )
        .unwrap();
    let retargeted = animation
        .update(
            MultiBufferOffset::new(20),
            bounds(200.0, 0.0),
            viewport(0.0),
            halfway,
        )
        .unwrap();
    assert_eq!(retargeted, current);
    let next = animation
        .update(
            MultiBufferOffset::new(20),
            bounds(200.0, 0.0),
            viewport(0.0),
            halfway + Duration::from_millis(40),
        )
        .unwrap();
    assert!(f32::from(next.left()) > f32::from(current.left()));
    assert!(f32::from(next.left()) < 200.0);
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
        MultiBufferOffset::new(10),
        bounds(100.0, 0.0),
        viewport(0.0),
        now,
    );
    let halfway = now + Duration::from_millis(40);
    let before = animation
        .update(
            MultiBufferOffset::new(10),
            bounds(100.0, 0.0),
            viewport(0.0),
            halfway,
        )
        .unwrap();
    let after = animation
        .update(
            MultiBufferOffset::new(10),
            bounds(100.0, -20.0),
            viewport(20.0),
            halfway,
        )
        .unwrap();
    assert_eq!(after.left(), before.left());
    assert_eq!(after.top(), before.top() - px(20.0));
    assert!(
        animation
            .update(
                MultiBufferOffset::new(10),
                bounds(100.0, -20.0),
                viewport(20.0),
                now + Duration::from_millis(80)
            )
            .is_none()
    );
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
    assert!(animation.transition.is_none());
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
        MultiBufferOffset::new(10),
        bounds(100.0, 0.0),
        viewport(0.0),
        now,
    );
    let resized = CursorViewport {
        bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(700.0), px(600.0))),
        scroll: point(px(0.0), px(0.0)),
    };
    assert!(
        animation
            .update(MultiBufferOffset::new(10), bounds(100.0, 0.0), resized, now)
            .is_none()
    );
    assert!(animation.transition.is_none());
}
