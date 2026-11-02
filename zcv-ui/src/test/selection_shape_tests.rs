use gpui::px;

use super::*;

fn contour(lines: &[(usize, f32, f32)]) -> Vec<SelectionContourLine> {
    lines
        .iter()
        .map(|&(index, start, end)| SelectionContourLine {
            index,
            start_x: px(start),
            end_x: px(end),
        })
        .collect()
}

#[test]
fn single_line_contour_rounds_all_four_corners() {
    let shapes = selection_segment_shapes(&contour(&[(0, 10., 110.)]), px(4.));
    assert_eq!(shapes.len(), 1);
    let shape = &shapes[0];
    // 圆角宽度封顶为行宽之半。
    for corner in shape.corners {
        assert_eq!(corner.style, CornerStyle::Round);
        assert_eq!(corner.width, px(4.));
    }
}

#[test]
fn narrow_segment_clamps_round_width_to_half_width() {
    // 行宽 6px 小于两倍圆角半径时，圆角宽度收敛为行宽之半。
    let shapes = selection_segment_shapes(&contour(&[(0, 0., 6.)]), px(4.));
    assert_eq!(shapes[0].corners[TOP_LEFT].width, px(3.));
}

#[test]
fn aligned_rows_keep_straight_inner_corners() {
    let shapes = selection_segment_shapes(&contour(&[(0, 10., 110.), (1, 10., 110.)]), px(4.));
    assert_eq!(shapes.len(), 2);
    // 首行：顶角圆、底角与下一行齐平 → 直角。
    assert_eq!(shapes[0].corners[TOP_LEFT].style, CornerStyle::Round);
    assert_eq!(shapes[0].corners[BOTTOM_LEFT].style, CornerStyle::Straight);
    assert_eq!(shapes[0].corners[BOTTOM_RIGHT].style, CornerStyle::Straight);
    // 末行：顶角齐平直角、底角圆。
    assert_eq!(shapes[1].corners[TOP_LEFT].style, CornerStyle::Straight);
    assert_eq!(shapes[1].corners[BOTTOM_RIGHT].style, CornerStyle::Round);
}

#[test]
fn stepped_rows_use_concave_and_convex_corners() {
    // 第二行右缘更窄：首行右下内凹，末行右上外凸。
    let shapes = selection_segment_shapes(&contour(&[(0, 10., 110.), (1, 10., 90.)]), px(4.));
    assert_eq!(shapes[0].corners[BOTTOM_RIGHT].style, CornerStyle::Concave);
    assert_eq!(shapes[1].corners[TOP_RIGHT].style, CornerStyle::Convex);
    // 左缘齐平 → 直角。
    assert_eq!(shapes[0].corners[BOTTOM_LEFT].style, CornerStyle::Straight);
    assert_eq!(shapes[1].corners[TOP_LEFT].style, CornerStyle::Straight);
}

#[test]
fn middle_row_of_three_rows_carries_steps_on_both_edges() {
    let shapes = selection_segment_shapes(
        &contour(&[(0, 10., 110.), (1, 0., 90.), (2, 20., 100.)]),
        px(4.),
    );
    // 中间行同时与上下行比较：四角都是阶梯角，无圆角。
    // 本行左缘 0 比上下行都更远（更宽）→ 左侧内凹；右缘 90 比上下行都更近（更窄）→ 右侧外凸。
    let middle = &shapes[1];
    assert_eq!(middle.corners[TOP_LEFT].style, CornerStyle::Concave);
    assert_eq!(middle.corners[TOP_RIGHT].style, CornerStyle::Convex);
    assert_eq!(middle.corners[BOTTOM_LEFT].style, CornerStyle::Concave);
    assert_eq!(middle.corners[BOTTOM_RIGHT].style, CornerStyle::Convex);
}

#[test]
fn empty_contour_produces_no_shapes() {
    assert!(selection_segment_shapes(&[], px(4.)).is_empty());
}

#[test]
fn corner_radius_scales_with_line_height() {
    assert_eq!(selection_corner_radius(px(26.)), px(3.9));
    assert_eq!(selection_line_end_overshoot(px(3.9)), px(7.8));
}
