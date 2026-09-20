use gpui::{point, px, size};

use super::*;

#[test]
fn reserves_at_least_four_line_number_digits() {
    let dimensions = GutterDimensions::line_numbers_only(9, px(8.), px(3.));

    assert_eq!(dimensions.crease_width, px(8.));
    assert_eq!(dimensions.left_padding, px(8.));
    assert_eq!(dimensions.right_padding, px(8.));
    assert_eq!(dimensions.width, px(56.));
    assert_eq!(dimensions.margin, px(3.));
    assert_eq!(dimensions.full_width(), px(59.));
}

#[test]
fn grows_after_the_reserved_digit_count_is_exceeded() {
    let four_digits = GutterDimensions::line_numbers_only(9_999, px(8.), px(-3.));
    let five_digits = GutterDimensions::line_numbers_only(10_000, px(8.), px(-3.));

    assert_eq!(five_digits.width - four_digits.width, px(8.));
    assert_eq!(five_digits.margin, px(3.));
}

#[test]
fn position_maps_to_the_nearest_visible_gutter_row() {
    let layout = GutterLayout {
        bounds: Bounds::new(point(px(0.), px(0.)), size(px(48.), px(100.))),
        line_height: px(20.),
        rows: vec![
            GutterRow {
                logical_line: Line::new(10),
                origin: point(px(20.), px(-5.)),
                shaped_line_number: ShapedLine::default(),
                crease: None,
            },
            GutterRow {
                logical_line: Line::new(11),
                origin: point(px(20.), px(15.)),
                shaped_line_number: ShapedLine::default(),
                crease: None,
            },
        ],
        crease_width: px(8.),
    };

    assert_eq!(
        layout.logical_line_for_position(point(px(4.), px(2.))),
        Some(Line::new(10))
    );
    assert_eq!(
        layout.logical_line_for_position(point(px(4.), px(22.))),
        Some(Line::new(11))
    );
    assert_eq!(
        layout.logical_line_for_position(point(px(60.), px(22.))),
        None
    );
}
