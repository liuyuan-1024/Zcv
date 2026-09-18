use super::*;

#[test]
fn snaps_height_to_complete_device_rows() {
    let layout = grid_layout(
        Bounds::new(Point::new(px(2.), px(3.)), size(px(100.), px(101.))),
        px(8.),
        px(20.),
        1.,
        false,
    );

    assert_eq!(layout.bounds.num_lines(), 5);
    assert_eq!(layout.origin, Point::new(px(2.), px(3.)));
}

#[test]
fn anchors_complete_rows_to_the_bottom_when_requested() {
    let layout = grid_layout(
        Bounds::new(Point::new(px(2.), px(3.)), size(px(100.), px(101.))),
        px(8.),
        px(20.),
        1.,
        true,
    );

    assert_eq!(layout.origin, Point::new(px(2.), px(4.)));
}
