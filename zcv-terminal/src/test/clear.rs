use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Point as AlacPoint},
};
use gpui::{AppContext as _, TestAppContext, px, size};
use zcv_settings::{SettingsStore, UserSettings};

use super::support::{display_only_terminal, write_output};
use crate::{
    Modes, Point, SelectionPoint, SelectionSide, SelectionType, Terminal, TerminalBounds,
    TerminalBuilder,
};

fn terminal_text(terminal: &Terminal) -> String {
    let term = terminal.term.lock();
    term.bounds_to_string(
        AlacPoint::new(term.topmost_line(), Column(0)),
        AlacPoint::new(term.bottommost_line(), term.last_column()),
    )
    .trim_end()
    .to_owned()
}

/// 清屏由模型完成：无历史、少量输出和回看历史时都只保留当前行。
#[gpui::test]
fn terminal_clear_keeps_only_current_line(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let terminal = cx.new(|cx| display_only_terminal(&TerminalBuilder::new(), cx));

    for old_lines in [0, 3, 20] {
        terminal.update(cx, |terminal, cx| {
            terminal.set_size(
                TerminalBounds::new(px(8.), px(16.), size(px(320.), px(96.))),
                cx,
            );
            terminal.sync(cx);
            write_output(terminal, b"\x1bc\x1b[?2004h", cx);
            write_output(
                terminal,
                "stale-output\r\n".repeat(old_lines).as_bytes(),
                cx,
            );
            // 光标下方也可能有旧内容，当前输入行不能成为清理范围的下界。
            write_output(terminal, b"\r\nstale-below\x1b[1A\r", cx);
            write_output(
                terminal,
                "\x1b[32m终端 % echo hello\x1b[0m\x1b[2D".as_bytes(),
                cx,
            );
            terminal.sync(cx);
            let before = terminal.last_content().clone();
            let current_line = before
                .cells
                .iter()
                .filter(|cell| cell.point.line == before.cursor.point.line)
                .map(|cell| cell.cell.clone())
                .collect::<Vec<_>>();
            if old_lines == 20 {
                assert!(terminal.term.lock().history_size() > 0);
                terminal.scroll_lines(10, cx);
                terminal.sync(cx);
                assert!(terminal.last_content().display_offset > 0);
            }

            terminal.select_range(
                SelectionType::Simple,
                SelectionPoint {
                    point: before.cursor.point,
                    side: SelectionSide::Left,
                },
                Some(SelectionPoint {
                    point: Point {
                        line: before.cursor.point.line,
                        column: before.cursor.point.column + 1,
                    },
                    side: SelectionSide::Right,
                }),
                cx,
            );
            terminal.sync(cx);
            assert!(terminal.last_content().selection.is_some());

            terminal.clear(cx);
            terminal.sync(cx);
            let content = terminal.last_content();
            assert!(content.selection.is_none());
            assert!(content.selection_text.is_none());
            assert_eq!(content.cursor.point.line, 0, "当前行应移到首行");
            assert_eq!(content.cursor.point.column, before.cursor.point.column);
            assert_eq!(content.mode, before.mode, "清屏不得重置终端模式");
            assert!(content.scrolled_to_top && content.scrolled_to_bottom);
            assert!(!terminal_text(terminal).contains("stale"));
            assert_eq!(
                content
                    .cells
                    .iter()
                    .take(content.columns)
                    .map(|cell| cell.cell.clone())
                    .collect::<Vec<_>>(),
                current_line,
                "当前行的字符、颜色和宽字符属性应保留"
            );

            let cleared = content.clone();
            terminal.scroll_lines(100, cx);
            terminal.sync(cx);
            assert_eq!(terminal.last_content().display_offset, 0);
            assert!(!terminal_text(terminal).contains("stale"));

            terminal.clear(cx);
            terminal.sync(cx);
            assert_eq!(terminal.last_content().cells, cleared.cells);
            // 后续输出应接着原来的光标位置写入，不能恢复旧屏幕。
            write_output(terminal, b"XY\r\nnext-output", cx);
            terminal.sync(cx);
            assert_eq!(terminal_text(terminal), "终端 % echo helXY\nnext-output");
            terminal.set_size(
                TerminalBounds::new(px(8.), px(16.), size(px(400.), px(160.))),
                cx,
            );
            terminal.sync(cx);
            assert!(!terminal_text(terminal).contains("stale"));
        });
    }
}

/// 自动折行属于同一逻辑行，包含历史中的前缀以及光标后的续行。
#[gpui::test]
fn terminal_clear_preserves_wrapped_logical_line(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let terminal = cx.new(|cx| display_only_terminal(&TerminalBuilder::new(), cx));
    let prompt =
        "liuyuan@MacBook-Air-2 演进计划超长目录名称你知道吗来测试一下啊.local % echo e\u{301}";

    for (columns, lines, move_cursor_up, alternate_screen) in [
        (40, 6, false, false),
        (17, 6, false, false),
        (17, 2, false, false),
        (17, 2, true, false),
        (40, 6, true, false),
        (40, 6, true, true),
    ] {
        terminal.update(cx, |terminal, cx| {
            terminal.set_size(
                TerminalBounds::new(
                    px(8.),
                    px(16.),
                    size(px(columns as f32 * 8.), px(lines as f32 * 16.)),
                ),
                cx,
            );
            terminal.sync(cx);
            write_output(terminal, b"\x1bc", cx);
            if alternate_screen {
                write_output(terminal, b"\x1b[?1049h", cx);
            }
            write_output(terminal, "stale-output\r\n".repeat(10).as_bytes(), cx);
            write_output(terminal, format!("\x1b[32m{prompt}\x1b[0m").as_bytes(), cx);
            if move_cursor_up {
                write_output(terminal, b"\x1b[1A", cx);
            }
            let (cursor, cursor_cell) = {
                let term = terminal.term.lock();
                let cursor = term.grid().cursor.point;
                (cursor, term.grid()[cursor].clone())
            };

            terminal.clear(cx);
            terminal.sync(cx);
            assert_eq!(
                terminal_text(terminal),
                prompt,
                "清屏必须保留整个折行提示符"
            );
            {
                let term = terminal.term.lock();
                let new_cursor = term.grid().cursor.point;
                assert_eq!(new_cursor.column, cursor.column);
                assert_eq!(term.grid()[new_cursor], cursor_cell, "光标应仍指向原字符");
            }
            let cleared = terminal.last_content().clone();
            terminal.clear(cx);
            terminal.sync(cx);
            assert_eq!(
                terminal.last_content(),
                &cleared,
                "重复清屏不得再次截断逻辑行"
            );

            // 备用屏幕不随列宽重排，尺寸变化后的布局由前台全屏程序重绘。
            if alternate_screen {
                return;
            }
            let expected = if move_cursor_up {
                prompt.to_owned()
            } else {
                write_output(terminal, b"Z", cx);
                format!("{prompt}Z")
            };
            terminal.set_size(
                TerminalBounds::new(px(8.), px(16.), size(px(640.), px(160.))),
                cx,
            );
            terminal.sync(cx);
            assert_eq!(
                terminal_text(terminal),
                expected,
                "继续输入和调整宽度后折行内容仍须完整"
            );
        });
    }
}

/// 备用屏幕也走同一清屏管线，退出后主屏幕仍由终端协议恢复。
#[gpui::test]
fn terminal_clear_preserves_alternate_screen_lifecycle(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let terminal = cx.new(|cx| display_only_terminal(&TerminalBuilder::new(), cx));

    terminal.update(cx, |terminal, cx| {
        terminal.set_size(
            TerminalBounds::new(px(8.), px(16.), size(px(320.), px(96.))),
            cx,
        );
        terminal.sync(cx);
        write_output(terminal, b"primary-screen\x1b[?1049h", cx);
        write_output(terminal, b"stale-alt\r\nactive-line", cx);
        terminal.sync(cx);
        assert!(terminal.last_content().mode.contains(Modes::ALT_SCREEN));

        terminal.clear(cx);
        terminal.sync(cx);
        let content = terminal.last_content();
        assert!(content.mode.contains(Modes::ALT_SCREEN));
        assert_eq!(
            content.cursor.point,
            Point {
                line: 0,
                column: 11
            }
        );
        assert_eq!(terminal_text(terminal), "active-line");
        assert!(content.scrolled_to_top && content.scrolled_to_bottom);

        write_output(terminal, b"!\x1b[?1049l", cx);
        terminal.sync(cx);
        let content = terminal.last_content();
        assert!(!content.mode.contains(Modes::ALT_SCREEN));
        assert_eq!(terminal_text(terminal), "primary-screen");
    });
}
