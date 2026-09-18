use gpui::{Modifiers, Point, px};

use super::*;

fn point(x: f32, y: f32) -> Point<Pixels> {
    Point::new(px(x), px(y))
}

fn bounds() -> (Point<Pixels>, Pixels, Pixels, usize, usize, usize) {
    (Point::new(px(0.), px(0.)), px(8.), px(16.), 0, 24, 80)
}

#[test]
fn grid_point_basic() {
    let (origin, cw, lh, offset, lines, cols) = bounds();
    let p = grid_point(point(16., 32.), origin, cw, lh, offset, lines, cols);
    assert_eq!(p, GridPoint { line: 2, column: 2 });
}

#[test]
fn grid_point_scrolled() {
    let (origin, cw, lh, _, lines, cols) = bounds();
    // display_offset=5：视口顶部绝对行 = -5。
    let p = grid_point(point(0., 0.), origin, cw, lh, 5, lines, cols);
    assert_eq!(p.line, -5);
}

#[test]
fn grid_point_clamped() {
    let (origin, cw, lh, _, lines, cols) = bounds();
    // 负 y（元素上方）钳制到视口顶行；超宽 x 钳制到最后一列。
    let p = grid_point(point(9999., -50.), origin, cw, lh, 0, lines, cols);
    assert_eq!(p.line, 0);
    assert_eq!(p.column, cols - 1);
}

#[test]
fn sgr_button_report() {
    let mode = Modes::SGR_MOUSE | Modes::MOUSE_REPORT_CLICK;
    let report = mouse_button_report(
        gpui::MouseButton::Left,
        &Modifiers::none(),
        GridPoint {
            line: 23,
            column: 2,
        },
        0,
        24,
        &mode,
        true,
    )
    .unwrap();
    assert_eq!(report, b"\x1b[<0;3;24M");
}

#[test]
fn mouse_report_disabled_without_mode() {
    assert!(
        mouse_button_report(
            gpui::MouseButton::Left,
            &Modifiers::none(),
            GridPoint { line: 0, column: 0 },
            0,
            24,
            &Modes::NONE,
            true,
        )
        .is_none()
    );
}

#[test]
fn alt_scroll_bytes() {
    assert_eq!(alt_scroll(2), b"\x1b[B\x1b[B");
    assert_eq!(alt_scroll(-1), b"\x1b[A");
}

#[test]
fn scroll_report_rows() {
    let mode = Modes::SGR_MOUSE | Modes::MOUSE_REPORT_CLICK;
    let reports = scroll_report(
        2,
        GridPoint {
            line: 23,
            column: 0,
        },
        0,
        24,
        &mode,
    )
    .unwrap();
    assert_eq!(reports.len(), 2);
    assert_eq!(reports[0], b"\x1b[<64;1;24M");
}
