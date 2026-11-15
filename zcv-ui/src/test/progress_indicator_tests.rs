use gpui::{point, px};

use super::arc_path;

#[test]
fn full_progress_draws_the_entire_ring() {
    let center = point(px(6.0), px(6.0));
    let full = arc_path(center, px(5.0), px(2.0), 0.0, 1.0).expect("应绘制完整圆环");

    assert!(full.bounds.left() < px(1.0));
    assert!(full.bounds.top() < px(1.0));
    assert!(full.bounds.right() > px(11.0));
    assert!(full.bounds.bottom() > px(11.0));
}

#[test]
fn a_quarter_ring_starts_at_the_top_and_rotates_clockwise() {
    let center = point(px(6.0), px(6.0));
    let first = arc_path(center, px(5.0), px(2.0), 0.0, 0.25).expect("应绘制右上方的四分之一圆弧");
    let next = arc_path(center, px(5.0), px(2.0), 0.25, 0.25).expect("应绘制右下方的四分之一圆弧");

    assert!(first.bounds.top() < px(1.0));
    assert!(first.bounds.bottom() < px(7.1));
    assert!(next.bounds.top() > px(4.9));
    assert!(next.bounds.bottom() > px(11.0));
    assert!(first.bounds.left() > px(4.9));
    assert!(next.bounds.left() > px(4.9));
}
