//! 像素坐标 → 网格坐标；鼠标/滚轮报告编码（SGR 与普通格式）。
//!
//! 仅在终端开启鼠标报告模式（MOUSE_MODE）时生成报告字节；否则返回 None，交给应用自身的选择/滚动逻辑。

use gpui::{MouseButton, Pixels, Point};

use crate::{Modes, Point as GridPoint};

/// 像素坐标 → 网格绝对坐标。
///
/// 视口顶部行绝对坐标为 -display_offset（与 display_iter 一致），视口行 i 的绝对行号 = i - display_offset；结果钳制到视口行范围。
pub(crate) fn grid_point(
    pos: Point<Pixels>,
    origin: Point<Pixels>,
    cell_width: Pixels,
    line_height: Pixels,
    display_offset: usize,
    screen_lines: usize,
    columns: usize,
) -> GridPoint {
    let column = ((pos.x - origin.x) / cell_width)
        .floor()
        .max(0.)
        .min(columns.saturating_sub(1) as f32) as usize;
    let viewport_line = ((pos.y - origin.y) / line_height).floor() as i32;
    // 视口顶部绝对坐标为 -display_offset，视口行 i 的绝对行 = i - display_offset。
    let topmost = -(display_offset as i32);
    let bottommost = topmost + screen_lines as i32 - 1;
    let line = (viewport_line - display_offset as i32).clamp(topmost, bottommost);
    GridPoint { line, column }
}

/// 像素坐标 → 网格坐标 + 选择侧边（格内位置决定 Left/Right）。
pub(crate) fn grid_point_and_side(
    pos: Point<Pixels>,
    origin: Point<Pixels>,
    cell_width: Pixels,
    line_height: Pixels,
    display_offset: usize,
    screen_lines: usize,
    columns: usize,
) -> (GridPoint, crate::SelectionSide) {
    let point = grid_point(
        pos,
        origin,
        cell_width,
        line_height,
        display_offset,
        screen_lines,
        columns,
    );
    let cell_x = pos.x - origin.x - point.column as f32 * cell_width;
    let side = if cell_x > cell_width / 2. {
        crate::SelectionSide::Right
    } else {
        crate::SelectionSide::Left
    };
    (point, side)
}

/// 鼠标报告字节（按下/释放）：SGR 或普通格式。
pub(crate) fn mouse_button_report(
    button: MouseButton,
    modifiers: &gpui::Modifiers,
    point: GridPoint,
    display_offset: usize,
    screen_lines: usize,
    mode: &Modes,
    clicked: bool,
) -> Option<Vec<u8>> {
    if !mode.intersects(Modes::MOUSE_MODE) {
        return None;
    }
    let button_code = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        _ => return None,
    };
    let modifier_code = mouse_modifier_code(modifiers);
    let suffix = if clicked { b'M' } else { b'm' };
    if mode.contains(Modes::SGR_MOUSE) {
        Some(
            format!(
                "\x1b[<{};{};{}{}",
                button_code + modifier_code,
                point.column + 1,
                viewport_line(point, display_offset, screen_lines) + 1,
                suffix as char
            )
            .into_bytes(),
        )
    } else {
        Some(normal_report(
            button_code + modifier_code,
            point,
            display_offset,
            screen_lines,
            suffix,
        ))
    }
}

/// 鼠标移动报告（MOTION/DRAG 模式）。
pub(crate) fn mouse_moved_report(
    button: Option<MouseButton>,
    modifiers: &gpui::Modifiers,
    point: GridPoint,
    display_offset: usize,
    screen_lines: usize,
    mode: &Modes,
) -> Option<Vec<u8>> {
    if !mode.intersects(Modes::MOUSE_MODE) {
        return None;
    }
    let dragging = button.is_some();
    if !(mode.contains(Modes::MOUSE_MOTION) || (dragging && mode.contains(Modes::MOUSE_DRAG))) {
        return None;
    }
    let base = if dragging { 32 } else { 35 };
    let button_code = match button {
        Some(MouseButton::Left) => base,
        Some(MouseButton::Middle) => base + 1,
        Some(MouseButton::Right) => base + 2,
        _ => base,
    };
    let modifier_code = mouse_modifier_code(modifiers);
    if mode.contains(Modes::SGR_MOUSE) {
        Some(
            format!(
                "\x1b[<{};{};{}M",
                button_code + modifier_code,
                point.column + 1,
                viewport_line(point, display_offset, screen_lines) + 1
            )
            .into_bytes(),
        )
    } else {
        Some(normal_report(
            button_code + modifier_code,
            point,
            display_offset,
            screen_lines,
            b'M',
        ))
    }
}

/// 滚轮报告：向上 64 / 向下 65，每行一个报告。
pub(crate) fn scroll_report(
    scroll_lines: i32,
    point: GridPoint,
    display_offset: usize,
    screen_lines: usize,
    mode: &Modes,
) -> Option<Vec<Vec<u8>>> {
    if !mode.intersects(Modes::MOUSE_MODE) {
        return None;
    }
    let direction = if scroll_lines < 0 { 65 } else { 64 };
    let count = scroll_lines.unsigned_abs() as usize;
    let viewport_line = viewport_line(point, display_offset, screen_lines) + 1;
    let mut reports = Vec::with_capacity(count);
    for _ in 0..count {
        if mode.contains(Modes::SGR_MOUSE) {
            reports.push(
                format!("\x1b[<{direction};{};{}M", point.column + 1, viewport_line).into_bytes(),
            );
        } else {
            reports.push(normal_report(
                direction,
                point,
                display_offset,
                screen_lines,
                b'M',
            ));
        }
    }
    Some(reports)
}

/// 备用屏幕下的滚轮回退：把滚轮转换为方向键字节。
pub(crate) fn alt_scroll(scroll_lines: i32) -> Vec<u8> {
    let direction = if scroll_lines < 0 { b'A' } else { b'B' };
    let count = scroll_lines.unsigned_abs() as usize;
    let mut bytes = Vec::with_capacity(count * 3);
    for _ in 0..count {
        bytes.extend_from_slice(&[0x1b, b'[', direction]);
    }
    bytes
}

/// 普通格式报告：`ESC [ M` + 编码字节（32 偏移，坐标带 33 偏移）。
fn normal_report(
    button: u32,
    point: GridPoint,
    display_offset: usize,
    screen_lines: usize,
    suffix: u8,
) -> Vec<u8> {
    let mut bytes = vec![0x1b, b'[', b'M'];
    bytes.push((32 + button) as u8);
    bytes.push((32 + point.column + 1) as u8);
    bytes.push((32 + viewport_line(point, display_offset, screen_lines) + 1) as u8);
    bytes.push(suffix);
    bytes
}

/// 视口行号（0 起）：绝对行 → 视口行。
///
/// 视口顶部行绝对坐标为 -display_offset，因此视口行 = 绝对行 + display_offset。
fn viewport_line(point: GridPoint, display_offset: usize, screen_lines: usize) -> usize {
    (point.line + display_offset as i32).clamp(0, screen_lines.saturating_sub(1) as i32) as usize
}

/// 鼠标报告修饰符码：shift=4、alt=8、ctrl=16。
fn mouse_modifier_code(modifiers: &gpui::Modifiers) -> u32 {
    let mut code = 0;
    if modifiers.shift {
        code += 4;
    }
    if modifiers.alt {
        code += 8;
    }
    if modifiers.control {
        code += 16;
    }
    code
}

#[cfg(test)]
mod tests {
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
}
