//! 编辑器鼠标命中测试：屏幕点 -> 文本 byte。

use gpui::{Bounds, Pixels, Point, ShapedLine};

pub(crate) struct PointerHitTest {
    lines: Vec<PointerHitLine>,
    gutter_offset: Pixels,
    scroll: Point<Pixels>,
    line_height: Pixels,
    top_adjusted: Pixels,
}

impl PointerHitTest {
    pub(crate) fn new(
        lines: Vec<PointerHitLine>,
        gutter_offset: Pixels,
        scroll: Point<Pixels>,
        line_height: Pixels,
        top_adjusted: Pixels,
    ) -> Self {
        Self {
            lines,
            gutter_offset,
            scroll,
            line_height,
            top_adjusted,
        }
    }

    pub(crate) fn byte_for_point(
        &self,
        point: Point<Pixels>,
        bounds: Bounds<Pixels>,
    ) -> Option<usize> {
        let line_height_f: f32 = self.line_height.into();
        if line_height_f <= 0.0 || self.lines.is_empty() {
            return None;
        }

        let y: f32 = (point.y - self.top_adjusted).into();
        let row = (y / line_height_f).floor() as isize;
        let row = row.clamp(0, self.lines.len() as isize - 1) as usize;
        let line = self.lines.get(row)?;

        let text_left = bounds.origin.x + self.gutter_offset - self.scroll.x;
        let x = point.x - text_left;
        let byte_in_line = line.shaped.closest_index_for_x(x).min(line.line_len);
        Some(line.line_start_byte + byte_in_line)
    }
}

pub(crate) struct PointerHitLine {
    line_start_byte: usize,
    line_len: usize,
    shaped: ShapedLine,
}

impl PointerHitLine {
    pub(crate) fn new(line_start_byte: usize, line_len: usize, shaped: ShapedLine) -> Self {
        Self {
            line_start_byte,
            line_len,
            shaped,
        }
    }
}
