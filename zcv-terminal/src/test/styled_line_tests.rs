use super::*;
use crate::{Point as TerminalPoint, alacritty::AlacrittyCell};
use alacritty_terminal::{
    term::cell::Flags,
    vte::ansi::{Color, NamedColor, Rgb},
};
use gpui::{Context, div};
use zcv_theme::ThemeChoice;

#[derive(Default)]
struct EmptyView;

impl gpui::Render for EmptyView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn cell(ch: char, fg: Color, bg: Color) -> IndexedCell {
    IndexedCell {
        point: TerminalPoint { line: 0, column: 0 },
        cell: Cell::new(AlacrittyCell {
            c: ch,
            fg,
            bg,
            flags: Flags::empty(),
            ..Default::default()
        }),
    }
}

fn cell_at(ch: char, column: usize, fg: Color, bg: Color) -> IndexedCell {
    IndexedCell {
        point: TerminalPoint { line: 0, column },
        cell: Cell::new(AlacrittyCell {
            c: ch,
            fg,
            bg,
            flags: Flags::empty(),
            ..Default::default()
        }),
    }
}

#[test]
fn terminal_style_maps_directly_to_text_run() {
    let foreground: Hsla = gpui::red();
    let background: Hsla = gpui::blue();
    let underline = gpui::UnderlineStyle {
        color: Some(foreground),
        thickness: px(1.),
        wavy: false,
    };
    let strikethrough = gpui::StrikethroughStyle {
        color: Some(foreground),
        thickness: px(1.),
    };
    let run = styled_text_run(
        TextRun {
            len: 3,
            font: gpui::font(".SystemUIFont"),
            color: Default::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        },
        HighlightStyle {
            color: Some(foreground),
            background_color: Some(background),
            font_weight: Some(gpui::FontWeight::BOLD),
            font_style: Some(gpui::FontStyle::Italic),
            underline: Some(underline),
            strikethrough: Some(strikethrough),
            ..Default::default()
        },
    );

    assert_eq!(run.len, 3);
    assert_eq!(run.color, foreground);
    assert_eq!(run.background_color, Some(background));
    assert_eq!(run.font.weight, gpui::FontWeight::BOLD);
    assert_eq!(run.font.style, gpui::FontStyle::Italic);
    assert_eq!(run.underline, Some(underline));
    assert_eq!(run.strikethrough, Some(strikethrough));
}

/// 宽字符渲染：force_width 强制每字形 1 格宽（CJK 字形 advance 恰好 1 格），宽字符占 2 格的间距由段起始列号 × 格宽定位补足（不补空格）。
/// "中"（列 0）"文"（列 2）"a"（列 4）三段渲染总宽应等于 5 格。
#[gpui::test]
fn wide_char_force_width_aligns_render_width(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        zcv_assets::Assets.load_fonts(cx).expect("内置字体应能加载");
    });
    let (_, cx) = cx.add_window_view(|window, cx| {
        ThemeChoice::System.apply(cx, Some(window));
        EmptyView
    });
    cx.update(|window, cx| {
        let font = typography::content_font();
        let font_size = typography::content_size(cx);
        let run = |ch: char| TextRun {
            len: ch.len_utf8(),
            font: font.clone(),
            color: color::current(cx).text.into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let cell_width = window
            .text_system()
            .shape_line("m".into(), font_size, &[run('m')], None)
            .width;
        // 前提：CJK 字形 advance 恰好 1 格（0.6em = 格宽），宽字符第二格由列号定位承担。
        let zhong = window
            .text_system()
            .shape_line("中".into(), font_size, &[run('中')], None)
            .width;
        assert!(
            (f32::from(zhong) - f32::from(cell_width)).abs() < 0.1,
            "CJK 字形应恰好 1 格：实际 {zhong:?}，格宽 {cell_width:?}"
        );
        // force_width 下每字形 1 格："中文" 2 字形 = 2 格。
        let forced = window
            .text_system()
            .shape_line(
                "中文".into(),
                font_size,
                &[run('中'), run('文')],
                Some(cell_width),
            )
            .width;
        assert!(
            (f32::from(forced) - f32::from(cell_width) * 2.0).abs() < 0.1,
            "force_width 下 '中文' 应等于 2 格：实际 {forced:?}，期望 {:?}",
            cell_width * 2.0
        );
        // 列号定位：段起点 [0, 2, 4]，各段 1 格宽 → 渲染总宽 5 格（网格 中+文+a = 5 列）。
        let segments = ["中", "文", "a"]
            .iter()
            .map(|ch| {
                window
                    .text_system()
                    .shape_line(
                        (*ch).into(),
                        font_size,
                        &[run(ch.chars().next().unwrap())],
                        Some(cell_width),
                    )
                    .width
            })
            .collect::<Vec<_>>();
        let starts = [0usize, 2, 4];
        let total = starts
            .iter()
            .zip(segments)
            .map(|(col, width)| *col as f32 * f32::from(cell_width) + f32::from(width))
            .fold(0.0f32, f32::max);
        assert!(
            (total - f32::from(cell_width) * 5.0).abs() < 0.1,
            "列号定位后宽字符行渲染宽度应等于 5 格：实际 {total:?}，期望 {:?}",
            cell_width * 5.0
        );
    });
}

/// 指纹对相同内容稳定、对内容变化敏感（行缓存失效判定的正确性）。
#[test]
fn row_fingerprint_is_stable_and_sensitive() {
    let red = Color::Spec(Rgb { r: 255, g: 0, b: 0 });
    let bg = Color::Named(NamedColor::Background);
    let same = || vec![cell('a', red, bg), cell('b', red, bg)];
    let changed = || vec![cell('a', red, bg), cell('c', red, bg)];
    assert_eq!(
        row_fingerprint(&same()),
        row_fingerprint(&same()),
        "相同内容指纹应一致"
    );
    assert_ne!(
        row_fingerprint(&same()),
        row_fingerprint(&changed()),
        "内容变化指纹应不同"
    );
}

/// 宽字符：占位格不产生文本；跨列切段并记录段起始列号（渲染按列定位），背景仍按格绘制。
#[gpui::test]
fn wide_char_spacer_skips_text_not_background(cx: &mut gpui::TestAppContext) {
    let (_, cx) = cx.add_window_view(|window, cx| {
        ThemeChoice::System.apply(cx, Some(window));
        EmptyView
    });
    let red = Color::Spec(Rgb { r: 255, g: 0, b: 0 });
    let bg = Color::Named(NamedColor::Background);
    let wide = |ch: char, column: usize| IndexedCell {
        point: TerminalPoint { line: 0, column },
        cell: Cell::new(AlacrittyCell {
            c: ch,
            fg: red,
            bg,
            flags: Flags::WIDE_CHAR,
            ..Default::default()
        }),
    };
    let spacer = |column: usize| IndexedCell {
        point: TerminalPoint { line: 0, column },
        cell: Cell::new(AlacrittyCell {
            c: ' ',
            fg: red,
            bg,
            flags: Flags::WIDE_CHAR_SPACER,
            ..Default::default()
        }),
    };
    let row = cx.update(|_window, cx| {
        let cells = vec![wide('中', 0), spacer(1), wide('文', 2), spacer(3)];
        row_to_styled_line(&cells, cx)
    });
    assert_eq!(row.text, "中文", "宽字符占位格不产生文本");
    assert_eq!(row.spans.len(), 2, "宽字符跨 2 列，相邻段按列切分");
    assert_eq!(row.spans[0].range, 0..3);
    assert_eq!(row.spans[1].range, 3..6);
    assert_eq!(
        row.span_columns.as_ref(),
        [0, 2],
        "段起始列号反映宽字符占 2 列（渲染按列号定位）"
    );
}

/// 相邻同样式格合并为一段；样式变化分段；尾随空格不参与 shaping。
#[gpui::test]
fn row_to_styled_line_merges_same_style_and_trims_trailing_spaces(cx: &mut gpui::TestAppContext) {
    let (_, cx) = cx.add_window_view(|window, cx| {
        ThemeChoice::System.apply(cx, Some(window));
        EmptyView
    });
    let red = Color::Spec(Rgb { r: 255, g: 0, b: 0 });
    let green = Color::Spec(Rgb { r: 0, g: 255, b: 0 });
    let bg = Color::Named(NamedColor::Background);
    let row = cx.update(|_window, cx| {
        let cells = vec![
            cell_at('a', 0, red, bg),
            cell_at('b', 1, red, bg),
            cell_at('c', 2, green, bg),
            cell_at(' ', 3, red, bg),
        ];
        row_to_styled_line(&cells, cx)
    });
    assert_eq!(row.text, "abc", "尾随空格应被裁剪");
    assert_eq!(row.spans.len(), 2, "相邻同样式格应合并为一段");
    assert_eq!(row.spans[0].range, 0..2);
    assert_eq!(row.spans[1].range, 2..3);
    assert_eq!(row.span_columns.as_ref(), [0, 2], "段起始列号按格连续推进");
    // 逆显格交换前景与背景。
    let inverse_row = cx.update(|_window, cx| {
        // 手工构造逆显格。
        let inverse = AlacrittyCell {
            c: 'y',
            fg: red,
            bg,
            flags: Flags::INVERSE,
            ..Default::default()
        };
        let cells = vec![
            cell('x', red, bg),
            IndexedCell {
                point: TerminalPoint { line: 0, column: 1 },
                cell: Cell::new(inverse),
            },
        ];
        row_to_styled_line(&cells, cx)
    });
    assert_eq!(inverse_row.spans.len(), 2, "逆显格样式不同应分段");
}
