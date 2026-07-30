//! Editor gutter 的尺寸与逐帧布局数据。
//!
//! Gutter 与正文共享垂直 DisplayRow 投影，但拥有独立的水平区域，不随正文横向滚动。

use gpui::{Bounds, Pixels, Point, ShapedLine, point};
use zcv_engine::Line;

/// 与 Zed 默认设置一致：至少预留四位行号，避免小文件增行时 gutter 频繁抖动。
pub(super) const MIN_LINE_NUMBER_DIGITS: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct GutterDimensions {
    pub(super) left_padding: Pixels,
    pub(super) right_padding: Pixels,
    pub(super) width: Pixels,
    pub(super) margin: Pixels,
}

impl GutterDimensions {
    pub(super) fn line_numbers_only(
        line_count: usize,
        digit_advance: Pixels,
        font_descent: Pixels,
    ) -> Self {
        let digit_count = decimal_digit_count(line_count.max(1)).max(MIN_LINE_NUMBER_DIGITS);
        let left_padding = digit_advance;
        let right_padding = digit_advance;
        Self {
            left_padding,
            right_padding,
            width: left_padding + digit_advance * digit_count + right_padding,
            // GPUI 不同后端对 descent 的符号约定不同，这里取其视觉距离。
            margin: font_descent.abs(),
        }
    }

    pub(super) fn full_width(self) -> Pixels {
        self.width + self.margin
    }
}

pub(super) struct GutterRow {
    pub(super) logical_line: Line,
    pub(super) origin: Point<Pixels>,
    pub(super) shaped_line_number: ShapedLine,
    pub(super) active: bool,
}

pub(super) struct GutterLayout {
    pub(super) bounds: Bounds<Pixels>,
    pub(super) line_height: Pixels,
    pub(super) rows: Vec<GutterRow>,
}

impl GutterLayout {
    /// 当前行背景贯穿 gutter 与正文，而不是只高亮行号区域。
    pub(super) fn active_row_bounds(
        &self,
        editor_right: Pixels,
    ) -> impl Iterator<Item = Bounds<Pixels>> + '_ {
        self.rows.iter().filter(|row| row.active).map(move |row| {
            Bounds::from_corners(
                point(self.bounds.left(), row.origin.y),
                point(editor_right, row.origin.y + self.line_height),
            )
        })
    }

    /// 将 gutter 中的像素位置映射到最近的可见逻辑行。
    pub(super) fn logical_line_for_position(&self, position: Point<Pixels>) -> Option<Line> {
        if !self.bounds.contains(&position) {
            return None;
        }
        let first = self.rows.first()?;
        let last = self.rows.last()?;
        if position.y <= first.origin.y {
            return Some(first.logical_line);
        }
        if position.y >= last.origin.y + self.line_height {
            return Some(last.logical_line);
        }
        self.rows
            .iter()
            .find(|row| position.y < row.origin.y + self.line_height)
            .map(|row| row.logical_line)
    }
}

fn decimal_digit_count(value: usize) -> usize {
    value.ilog10() as usize + 1
}

#[cfg(test)]
mod tests {
    use gpui::{point, px, size};

    use super::*;

    #[test]
    fn reserves_at_least_four_line_number_digits() {
        let dimensions = GutterDimensions::line_numbers_only(9, px(8.), px(3.));

        assert_eq!(dimensions.left_padding, px(8.));
        assert_eq!(dimensions.right_padding, px(8.));
        assert_eq!(dimensions.width, px(48.));
        assert_eq!(dimensions.margin, px(3.));
        assert_eq!(dimensions.full_width(), px(51.));
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
                    active: false,
                },
                GutterRow {
                    logical_line: Line::new(11),
                    origin: point(px(20.), px(15.)),
                    shaped_line_number: ShapedLine::default(),
                    active: true,
                },
            ],
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
        assert_eq!(
            layout.active_row_bounds(px(300.)).collect::<Vec<_>>(),
            vec![Bounds::new(point(px(0.), px(15.)), size(px(300.), px(20.)))]
        );
    }
}
