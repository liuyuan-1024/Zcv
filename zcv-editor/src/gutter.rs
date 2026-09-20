//! Editor gutter 的尺寸与逐帧布局数据。
//!
//! Gutter 与正文共享垂直 DisplayRow 投影，但拥有独立的水平区域，不随正文横向滚动。

use gpui::{Bounds, Pixels, Point, ShapedLine};
use zcv_text::Line;

/// 至少预留四位行号，避免小文件增行时 gutter 频繁抖动。
pub(super) const MIN_LINE_NUMBER_DIGITS: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct GutterDimensions {
    /// 折叠指示列宽（crease 绘制在行号右侧；gutter 左侧留给 git 状态竖条）。
    pub(super) crease_width: Pixels,
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
        let crease_width = digit_advance;
        let left_padding = digit_advance;
        let right_padding = digit_advance;
        Self {
            crease_width,
            left_padding,
            right_padding,
            width: left_padding + digit_advance * digit_count + right_padding + crease_width,
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
    /// 折叠指示：None 不可折叠；Some(folded) 可折叠（已折叠时显示展开箭头）。
    pub(super) crease: Option<bool>,
}

pub(super) struct GutterLayout {
    pub(super) bounds: Bounds<Pixels>,
    pub(super) line_height: Pixels,
    pub(super) rows: Vec<GutterRow>,
    /// 折叠指示列宽（crease 箭头绘制与点击 hitbox 使用）。
    pub(super) crease_width: Pixels,
}

impl GutterLayout {
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
#[path = "test/gutter_tests.rs"]
mod tests;
