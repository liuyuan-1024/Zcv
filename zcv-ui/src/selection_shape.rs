//! 选区形状原语：连续行轮廓 → 每行四角样式 → 片段绘制。
//!
//! Editor 与 Markdown 预览共用同一套形状语义（圆角半径、行阶梯、行尾延伸），保证两边选区视觉一致。
//! 调用方负责把各自的选区投影为连续行轮廓：行间隙或块间隙处应断开轮廓，断开的轮廓各自应用首末行圆角。

use gpui::{Bounds, Hsla, PathBuilder, Pixels, Window, fill, point};

/// 片段四角索引：左上、右上、右下、左下。
pub const TOP_LEFT: usize = 0;
pub const TOP_RIGHT: usize = 1;
pub const BOTTOM_RIGHT: usize = 2;
pub const BOTTOM_LEFT: usize = 3;

/// 片段角样式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CornerStyle {
    /// 直角（内部接缝与普通背景片段）。
    Straight,
    /// 外角圆角（选区轮廓首末行）。
    Round,
    /// 内凹切角（阶梯处更宽一行）。
    Concave,
    /// 外凸补角（阶梯处更窄一行）。
    Convex,
}

/// 角样式与曲线横向跨度（圆角取轮廓行宽之半，阶梯角取两行外缘差之半，均封顶圆角半径）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corner {
    pub style: CornerStyle,
    pub width: Pixels,
}

pub(crate) const STRAIGHT_CORNER: Corner = Corner {
    style: CornerStyle::Straight,
    width: Pixels::ZERO,
};

pub const ALL_STRAIGHT: [Corner; 4] = [STRAIGHT_CORNER; 4];

/// 选区轮廓的一行：调用方定义的行索引与行内水平区间。
#[derive(Debug)]
pub struct SelectionContourLine {
    pub index: usize,
    pub start_x: Pixels,
    pub end_x: Pixels,
}

/// 带角样式的选区片段（一行）：角样式携带轮廓上下文。
#[derive(Clone, Debug)]
pub struct SelectionSegmentShape {
    pub index: usize,
    pub start_x: Pixels,
    pub end_x: Pixels,
    pub corners: [Corner; 4],
}

/// 选区圆角半径：行高的 15%。
pub fn selection_corner_radius(line_height: Pixels) -> Pixels {
    line_height * 0.15
}

/// 跨行选区的行尾延伸量：让连续行的选区在视觉上衔接。
pub fn selection_line_end_overshoot(corner_radius: Pixels) -> Pixels {
    corner_radius * 2.
}

/// 阶梯角：本行外缘比邻行更远（右缘更大、左缘更小）→ 本行更宽 → 内凹；反之外凸；齐平 → 直角。
fn step_corner(own: Pixels, other: Pixels, right_side: bool, corner_radius: Pixels) -> Corner {
    if own == other {
        return STRAIGHT_CORNER;
    }
    let width = if own > other {
        own - other
    } else {
        other - own
    };
    let wider = if right_side { own > other } else { own < other };
    Corner {
        style: if wider {
            CornerStyle::Concave
        } else {
            CornerStyle::Convex
        },
        width: (width / 2.).min(corner_radius),
    }
}

/// 按轮廓上下文为每行计算四角样式：首行外角圆角、末行下外角圆角，行阶梯处宽行内凹/窄行外凸。
///
/// 轮廓必须只含连续行；行间隙处由调用方断开为多个轮廓。
pub fn selection_segment_shapes(
    contour: &[SelectionContourLine],
    corner_radius: Pixels,
) -> Vec<SelectionSegmentShape> {
    let count = contour.len();
    (0..count)
        .map(|index| {
            let (line_ix, start_x, end_x) = {
                let line = &contour[index];
                (line.index, line.start_x, line.end_x)
            };
            let round_width = ((end_x - start_x) / 2.).min(corner_radius);
            let mut corners = ALL_STRAIGHT;
            if index == 0 {
                corners[TOP_LEFT] = Corner {
                    style: CornerStyle::Round,
                    width: round_width,
                };
                corners[TOP_RIGHT] = Corner {
                    style: CornerStyle::Round,
                    width: round_width,
                };
            } else {
                let (_, prev_start, prev_end) = {
                    let previous = &contour[index - 1];
                    (previous.index, previous.start_x, previous.end_x)
                };
                corners[TOP_LEFT] = step_corner(start_x, prev_start, false, corner_radius);
                corners[TOP_RIGHT] = step_corner(end_x, prev_end, true, corner_radius);
            }
            if index == count - 1 {
                corners[BOTTOM_LEFT] = Corner {
                    style: CornerStyle::Round,
                    width: round_width,
                };
                corners[BOTTOM_RIGHT] = Corner {
                    style: CornerStyle::Round,
                    width: round_width,
                };
            } else {
                let (_, next_start, next_end) = {
                    let next = &contour[index + 1];
                    (next.index, next.start_x, next.end_x)
                };
                corners[BOTTOM_LEFT] = step_corner(start_x, next_start, false, corner_radius);
                corners[BOTTOM_RIGHT] = step_corner(end_x, next_end, true, corner_radius);
            }
            SelectionSegmentShape {
                index: line_ix,
                start_x,
                end_x,
                corners,
            }
        })
        .collect()
}

/// 绘制选区片段：全直角走矩形快速路径，带角样式按四角规则构建轮廓路径。
///
/// bounds 为片段的行盒（高度覆盖整个行高）。
pub fn paint_selection_segment(
    bounds: Bounds<Pixels>,
    corners: [Corner; 4],
    corner_radius: Pixels,
    color: Hsla,
    window: &mut Window,
) {
    let top_left = bounds.origin;
    let top_right = point(bounds.right(), bounds.origin.y);
    let bottom_right = bounds.bottom_right();
    let bottom_left = point(bounds.origin.x, bounds.bottom());
    let curve_height = point(Pixels::ZERO, corner_radius);
    let [
        top_left_corner,
        top_right_corner,
        bottom_right_corner,
        bottom_left_corner,
    ] = corners;

    if corners
        .iter()
        .all(|corner| corner.style == CornerStyle::Straight)
    {
        window.paint_quad(fill(Bounds::from_corners(top_left, bottom_right), color));
        return;
    }

    // 顺时针：左上 → 顶边 → 右上 → 右边 → 右下 → 底边 → 左下 → 左边。
    let mut builder = PathBuilder::fill();
    builder.move_to(match top_left_corner.style {
        CornerStyle::Round | CornerStyle::Concave => {
            top_left + point(top_left_corner.width, Pixels::ZERO)
        }
        CornerStyle::Convex => top_left - point(top_left_corner.width, Pixels::ZERO),
        CornerStyle::Straight => top_left,
    });
    match top_right_corner.style {
        CornerStyle::Straight => builder.line_to(top_right),
        CornerStyle::Round | CornerStyle::Concave => {
            builder.line_to(top_right - point(top_right_corner.width, Pixels::ZERO));
            builder.curve_to(top_right + curve_height, top_right);
        }
        CornerStyle::Convex => {
            builder.line_to(top_right + point(top_right_corner.width, Pixels::ZERO));
            builder.curve_to(top_right + curve_height, top_right);
        }
    }
    match bottom_right_corner.style {
        CornerStyle::Straight => builder.line_to(bottom_right),
        CornerStyle::Round | CornerStyle::Concave => {
            builder.line_to(bottom_right - curve_height);
            builder.curve_to(
                bottom_right - point(bottom_right_corner.width, Pixels::ZERO),
                bottom_right,
            );
        }
        CornerStyle::Convex => {
            builder.line_to(bottom_right - curve_height);
            builder.curve_to(
                bottom_right + point(bottom_right_corner.width, Pixels::ZERO),
                bottom_right,
            );
        }
    }
    match bottom_left_corner.style {
        CornerStyle::Straight => builder.line_to(bottom_left),
        CornerStyle::Round | CornerStyle::Concave => {
            builder.line_to(bottom_left + point(bottom_left_corner.width, Pixels::ZERO));
            builder.curve_to(bottom_left - curve_height, bottom_left);
        }
        CornerStyle::Convex => {
            builder.line_to(bottom_left - point(bottom_left_corner.width, Pixels::ZERO));
            builder.curve_to(bottom_left - curve_height, bottom_left);
        }
    }
    match top_left_corner.style {
        CornerStyle::Straight => builder.line_to(top_left),
        CornerStyle::Round | CornerStyle::Concave => {
            builder.line_to(top_left + curve_height);
            builder.curve_to(
                top_left + point(top_left_corner.width, Pixels::ZERO),
                top_left,
            );
        }
        CornerStyle::Convex => {
            builder.line_to(top_left + curve_height);
            builder.curve_to(
                top_left - point(top_left_corner.width, Pixels::ZERO),
                top_left,
            );
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

#[cfg(test)]
#[path = "test/selection_shape_tests.rs"]
mod tests;
