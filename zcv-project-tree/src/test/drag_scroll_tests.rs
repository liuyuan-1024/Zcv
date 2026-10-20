use gpui::{Bounds, Pixels, point, px, size};

use super::drag_scroll_step;

#[test]
fn project_tree_drag_scroll_zone_scales_with_panel_height() {
    for height in [px(200.), px(800.)] {
        let bounds = Bounds {
            origin: point(px(10.), px(20.)),
            size: size(px(300.), height),
        };
        let y = |fraction: f32| bounds.origin.y + height * fraction;

        assert_eq!(drag_scroll_step(point(px(100.), y(0.04)), bounds), px(8.));
        assert_eq!(drag_scroll_step(point(px(100.), y(0.1)), bounds), px(5.));
        assert_eq!(
            drag_scroll_step(point(px(100.), y(0.5)), bounds),
            Pixels::ZERO
        );
        assert_eq!(drag_scroll_step(point(px(100.), y(0.9)), bounds), px(-5.));
        assert_eq!(drag_scroll_step(point(px(100.), y(0.97)), bounds), px(-8.));
        assert_eq!(
            drag_scroll_step(point(px(100.), bounds.origin.y - px(1.)), bounds),
            Pixels::ZERO
        );
    }
}

#[test]
fn project_tree_drag_scroll_offset_uses_negative_list_range() {
    let max_offset = px(100.);
    let current = px(-40.);
    let next = (current + px(12.)).min(px(0.)).max(-max_offset);
    assert_eq!(next, px(-28.));

    let at_top = (px(-4.) + px(12.)).min(px(0.)).max(-max_offset);
    assert_eq!(at_top, px(0.));

    let at_bottom = (px(-96.) - px(12.)).min(px(0.)).max(-max_offset);
    assert_eq!(at_bottom, px(-100.));
}
