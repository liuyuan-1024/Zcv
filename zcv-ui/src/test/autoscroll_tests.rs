use super::*;
use gpui::{point, px, size};

#[test]
fn scrolls_proportionally_near_each_edge() {
    let bounds = Bounds {
        origin: point(px(0.), px(0.)),
        size: size(px(800.), px(200.)),
    };
    let edge_margin = point(px(20.), px(20.));
    let max_delta = point(px(50.), px(12.5));

    assert_eq!(
        drag_autoscroll_delta(point(px(100.), px(100.)), bounds, edge_margin, max_delta),
        point(Pixels::ZERO, Pixels::ZERO)
    );
    assert_eq!(
        drag_autoscroll_delta(point(px(100.), px(-100.)), bounds, edge_margin, max_delta),
        point(px(0.), px(12.5))
    );
    assert_eq!(
        drag_autoscroll_delta(point(px(100.), px(300.)), bounds, edge_margin, max_delta),
        point(px(0.), px(-12.5))
    );
    assert_eq!(
        drag_autoscroll_delta(point(px(-10.), px(100.)), bounds, edge_margin, max_delta),
        point(px(-9.), px(0.))
    );
    assert_eq!(
        drag_autoscroll_delta(point(px(810.), px(100.)), bounds, edge_margin, max_delta),
        point(px(9.), px(0.))
    );
}
