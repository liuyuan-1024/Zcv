use super::*;
use crate::{Cursor, Modes, Point, alacritty::AlacrittyCell};

/// 光标行是视口相对行：滚动（display_offset 增大）时光标随内容上移，而不是停留在原屏幕位置。
#[test]
fn cursor_row_is_viewport_relative_and_clamped() {
    let content = |display_offset: usize, line: i32| Content {
        cells: Vec::new(),
        mode: Modes::NONE,
        total_lines: 100,
        display_offset,
        columns: 80,
        screen_lines: 30,
        selection_text: None,
        selection: None,
        cursor: Cursor {
            shape: CursorShape::Block,
            point: Point { line, column: 3 },
        },
        cursor_cell: Cell::new(AlacrittyCell {
            c: 'x',
            ..Default::default()
        }),
        terminal_bounds: TerminalBounds::default(),
        scrolled_to_top: false,
        scrolled_to_bottom: false,
        bottom_row_occupied: false,
    };
    // 滚动 10 行后视口行 5 的光标仍定位在第 5 行（若错误叠加偏移会得到 15）。
    assert_eq!(cursor_row(&content(10, 5)), Some(5));
    // 光标滚出视口（行号越界）时隐藏而非钳制悬浮。
    assert_eq!(cursor_row(&content(0, 50)), None);
    assert_eq!(cursor_row(&content(0, -3)), None);
}
