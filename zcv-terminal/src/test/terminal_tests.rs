use alacritty_terminal::{
    event::VoidListener,
    grid::Scroll as AlacScroll,
    term::{Term, test::mock_term},
};

use super::*;

#[test]
fn terminal_lifecycle_has_explicit_close_and_exit_transitions() {
    let mut lifecycle = TerminalLifecycle::Running;
    assert!(lifecycle.begin_close());
    assert_eq!(lifecycle, TerminalLifecycle::Closing);
    assert!(lifecycle.begin_close());
    assert!(lifecycle.mark_exited());
    assert_eq!(lifecycle, TerminalLifecycle::Exited);
    assert!(!lifecycle.begin_close());
    assert!(!lifecycle.mark_exited());
}

fn content_of(term: &Term<VoidListener>) -> Content {
    alacritty::make_content(term, None)
}

/// 快照基本结构：单行文本的单元格与坐标。
#[test]
fn content_snapshot_basic() {
    let term = mock_term("hello");
    let content = content_of(&term);
    assert_eq!(content.cells.len(), 5);
    // 内容从第 0 行开始，逐列排列。
    assert_eq!(content.cells[0].point.line, 0);
    assert_eq!(content.cells[0].point.column, 0);
    assert_eq!(content.cells[0].cell.character(), 'h');
    assert_eq!(content.cells[4].cell.character(), 'o');
    assert_eq!(content.columns, 5);
    assert_eq!(content.screen_lines, 1);
    assert!(content.scrolled_to_bottom);
    assert!(content.scrolled_to_top);
    assert!(content.bottom_row_occupied);
}

/// 光标与内容的绝对坐标换算：mock_term 直接写网格，光标保持初始位置 (0, 0)。
#[test]
fn cursor_absolute_coordinates() {
    let term = mock_term("hi\n");
    let content = content_of(&term);
    let cursor = content.cursor;
    assert_eq!(cursor.point.line, 0);
    assert_eq!(cursor.point.column, 0);
    assert_eq!(content.cursor_cell.character(), 'h');
}

/// 宽字符：WIDE_CHAR 与 WIDE_CHAR_SPACER 标记保留。
#[test]
fn wide_char_cells() {
    let term = mock_term("你好");
    let content = content_of(&term);
    let cells: Vec<_> = content.cells.iter().collect();
    assert_eq!(cells.len(), 4);
    assert_eq!(cells[0].cell.character(), '你');
    assert!(cells[0].cell.is_wide_char());
    assert!(cells[1].cell.is_wide_char_spacer());
}

/// 滚动映射往返。
#[test]
fn scroll_mapping() {
    assert!(matches!(
        Scroll::Delta(3).to_alacritty(),
        AlacScroll::Delta(3)
    ));
    assert!(matches!(Scroll::Bottom.to_alacritty(), AlacScroll::Bottom));
}

/// Modes 从 alacritty 模式映射：默认模式应包含 SHOW_CURSOR 等。
#[test]
fn modes_from_alacritty() {
    let mode = alacritty_terminal::term::TermMode::default();
    let modes = Modes::from_alacritty(mode);
    assert!(modes.contains(Modes::SHOW_CURSOR));
    assert!(modes.contains(Modes::LINE_WRAP));
    assert!(!modes.contains(Modes::ALT_SCREEN));
}

/// 像素尺寸换算：行/列数向下取整并容忍浮点误差。
#[test]
fn terminal_bounds_dims() {
    let bounds = TerminalBounds::new(
        Pixels::from(8.),
        Pixels::from(16.),
        Size {
            width: Pixels::from(100.),
            height: Pixels::from(50.),
        },
    );
    assert_eq!(bounds.num_columns(), 12);
    assert_eq!(bounds.num_lines(), 3);
}

#[test]
fn terminal_typography_uses_its_own_configured_size() {
    let mut user_settings = UserSettings::default();
    let default = TerminalSettings::from_user_settings(&user_settings, None);
    assert_eq!(default.font_size, 16.);
    assert_eq!(default.line_height, 1.618);

    user_settings.terminal_font_size = 14.;
    user_settings.terminal_line_height = 1.2;
    let configured = TerminalSettings::from_user_settings(&user_settings, None);
    assert_eq!(configured.font_size, 14.);
    assert_eq!(configured.line_height, 1.2);
}

#[test]
fn terminal_typography_override_is_isolated_per_session() {
    let settings = UserSettings::default();
    let first = TerminalSettings::from_user_settings(&settings, Some(20.));
    let second = TerminalSettings::from_user_settings(&settings, None);

    assert_eq!(first.font_size, 20.);
    assert_eq!(second.font_size, settings.terminal_font_size);
}

/// 选择范围包含判断。
#[test]
fn selection_range_contains() {
    let range = SelectionRange {
        start: Point { line: 0, column: 1 },
        end: Point { line: 1, column: 3 },
        is_block: false,
    };
    assert!(range.contains(Point { line: 0, column: 1 }));
    assert!(range.contains(Point { line: 1, column: 3 }));
    assert!(!range.contains(Point { line: 0, column: 0 }));
    assert!(!range.contains(Point { line: 2, column: 0 }));
}
