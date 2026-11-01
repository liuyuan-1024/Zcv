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
        point(px(9.), px(0.))
    );
    assert_eq!(
        drag_autoscroll_delta(point(px(810.), px(100.)), bounds, edge_margin, max_delta),
        point(px(-9.), px(0.))
    );
}

#[test]
fn selection_autoscroll_only_uses_exceeded_edges() {
    let bounds = Bounds::new(point(px(20.), px(30.)), size(px(400.), px(200.)));
    let line_height = px(20.);
    assert_eq!(
        selection_autoscroll_delta(point(px(419.), px(120.)), bounds, line_height),
        point(px(0.), px(0.))
    );
    let vertical = selection_autoscroll_delta(point(px(419.), px(240.)), bounds, line_height);
    assert_eq!(vertical.x, px(0.));
    assert!(vertical.y < px(0.));
    let horizontal = selection_autoscroll_delta(point(px(430.), px(120.)), bounds, line_height);
    assert!(horizontal.x < px(0.));
    assert_eq!(horizontal.y, px(0.));
    let diagonal = selection_autoscroll_delta(point(px(430.), px(240.)), bounds, line_height);
    assert!(diagonal.x < px(0.) && diagonal.y < px(0.));
}
