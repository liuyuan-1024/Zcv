use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AppContext, Context, IntoElement, Modifiers, ParentElement, Render, StyledText, TestAppContext,
    Window, div, point, px, size,
};
use zcv_editor::Editor;
use zcv_theme::typography;
use zcv_workspace::PreviewDocument;

use crate::document::{Inline, InlineStyle, parse};

use super::{
    Block, MARKDOWN_REPARSE_DEBOUNCE, MarkdownInlineText, MarkdownPreviewView, OpenPathCallback,
    code_lines, heading_line_height, heading_size, highlight_code_blocks, list_marker_char_count,
    render_math, render_text_inline, resolve_markdown_file_link, visible_highlights_for_line,
};
use zcv_language::{HighlightSpan, LanguageRegistry, SnippetHighlightCancellation};

fn plain(text: &str) -> Inline {
    Inline {
        text: text.into(),
        style: InlineStyle::default(),
    }
}

#[test]
fn code_lines_omits_parser_terminator_without_dropping_blank_lines() {
    assert_eq!(
        code_lines("let x = 1;\n").collect::<Vec<_>>(),
        ["let x = 1;"]
    );
    assert_eq!(
        code_lines("let x = 1;\n\n").collect::<Vec<_>>(),
        ["let x = 1;", ""]
    );
}

#[test]
fn resolves_relative_file_links_against_the_markdown_directory() {
    let directory = Path::new("/project/docs");

    assert_eq!(
        resolve_markdown_file_link("../src/main.rs#main", Some(directory)),
        Some(PathBuf::from("/project/docs/../src/main.rs"))
    );
    assert_eq!(
        resolve_markdown_file_link("https://zcv.dev", Some(directory)),
        None
    );
    assert_eq!(
        resolve_markdown_file_link("#section", Some(directory)),
        None
    );
}

#[gpui::test]
fn clicking_relative_file_link_uses_the_workspace_opener(cx: &mut TestAppContext) {
    struct TestWindow {
        content: Vec<Inline>,
        directory: PathBuf,
        open_path: OpenPathCallback,
    }

    impl Render for TestWindow {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().child(render_text_inline(
                &self.content,
                0,
                Some(self.directory.as_path()),
                Some(&self.open_path),
                _cx,
            ))
        }
    }

    let opened_path = Rc::new(RefCell::new(None));
    let open_path: OpenPathCallback = {
        let opened_path = opened_path.clone();
        Rc::new(move |path, _, _| {
            *opened_path.borrow_mut() = Some(path);
        })
    };
    let blocks = parse("[Rust 示例](./main.rs)");
    let Block::Paragraph(content) = blocks[0].clone() else {
        panic!("应解析为段落");
    };
    let directory = PathBuf::from("/project/docs");
    let (_, cx) = cx.add_window_view(move |_, _| TestWindow {
        content,
        directory,
        open_path,
    });
    cx.refresh().expect("测试窗口应完成首次绘制");
    cx.simulate_click(point(px(12.), px(8.)), Modifiers::none());

    assert_eq!(
        opened_path.borrow().as_deref(),
        Some(Path::new("/project/docs/./main.rs"))
    );
}

#[test]
fn code_line_highlights_scan_each_span_once_except_for_its_covered_lines() {
    let spans = [
        HighlightSpan {
            range: 2..5,
            capture: 1,
        },
        HighlightSpan {
            range: 8..14,
            capture: 2,
        },
        HighlightSpan {
            range: 15..17,
            capture: 3,
        },
    ];
    let mut index = 0;

    assert_eq!(
        visible_highlights_for_line(&spans, 0..4, &mut index),
        vec![(2..4, 1)]
    );
    assert_eq!(
        visible_highlights_for_line(&spans, 5..9, &mut index),
        vec![(3..4, 2)]
    );
    assert_eq!(
        visible_highlights_for_line(&spans, 10..14, &mut index),
        vec![(0..4, 2)]
    );
    assert_eq!(
        visible_highlights_for_line(&spans, 15..18, &mut index),
        vec![(0..2, 3)]
    );
    assert_eq!(index, spans.len());
}

#[test]
fn ordered_list_marker_width_accounts_for_multi_digit_numbers() {
    assert_eq!(list_marker_char_count(None, 3), 1);
    assert_eq!(list_marker_char_count(Some(1), 9), 2);
    assert_eq!(list_marker_char_count(Some(10), 11), 3);
    assert_eq!(list_marker_char_count(Some(98), 3), 4);
}

#[gpui::test]
fn headings_preserve_a_minimum_line_height_at_their_own_font_size(cx: &mut TestAppContext) {
    let type_scale = cx.update(|cx| typography::current(cx));
    for level in 1..=6 {
        let size = heading_size(level, type_scale);
        assert!(heading_line_height(level, size, type_scale) >= size * 1.2);
    }
}

#[gpui::test]
fn inline_code_chip_outsets_text_and_uses_rounded_background(cx: &mut TestAppContext) {
    struct TestWindow;

    impl Render for TestWindow {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    let (_, cx) = cx.add_window_view(|_, _| TestWindow);
    let chip_color = gpui::red();
    let text = StyledText::new("a xxx b");
    let layout = text.layout().clone();
    let expected_layout = layout.clone();

    cx.draw(Default::default(), size(px(400.), px(100.)), |_, _| {
        MarkdownInlineText {
            text: text.into_any_element(),
            layout,
            code_ranges: std::iter::once(2..5).collect(),
            code_color: chip_color,
        }
    });

    let code_start = expected_layout
        .position_for_index(2)
        .expect("行内代码起点应存在");
    let code_end = expected_layout
        .position_for_index(5)
        .expect("行内代码终点应存在");
    let content_width = code_end.x - code_start.x;

    let chip = cx.update(|window, _| {
        window
            .painted_quads()
            .into_iter()
            .find(|quad| quad.background == chip_color.into())
            .expect("应绘制行内代码背景")
    });
    assert!(
        chip.bounds.size.width.as_f32() > content_width.as_f32(),
        "行内代码背景应比文字本身更宽"
    );
    assert!(
        chip.corner_radii.top_left.as_f32() > 0.,
        "行内代码背景应使用圆角"
    );
}

#[gpui::test]
fn math_preview_size_follows_content_font_size(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let renderer = cx.svg_renderer();
        let small = render_math(
            "x^2",
            false,
            16.,
            ratex_types::color::Color::WHITE,
            renderer.clone(),
        )
        .expect("有效公式应能渲染");
        let large = render_math(
            "x^2",
            false,
            32.,
            ratex_types::color::Color::WHITE,
            renderer,
        )
        .expect("有效公式应能渲染");
        assert!(
            large.size(0).width > small.size(0).width,
            "公式图像应随内容字号放大"
        );
        assert!(
            large.size(0).height > small.size(0).height,
            "公式图像高度应随内容字号放大"
        );
    });
}

#[test]
fn applies_language_highlights_to_fenced_code_blocks() {
    let mut blocks = parse("```rust\nfn main() {}\n```");
    assert!(highlight_code_blocks(
        &mut blocks,
        &Arc::new(LanguageRegistry::new()),
        &SnippetHighlightCancellation::default()
    ));
    assert!(matches!(
        blocks.as_slice(),
        [Block::Code {
            highlights: Some(highlights),
            ..
        }] if !highlights.spans.is_empty()
    ));
}

#[gpui::test]
fn preview_rebuilds_when_source_document_changes(cx: &mut TestAppContext) {
    let editor = cx.new(Editor::single_line);
    editor.update(cx, |editor, cx| {
        editor.set_text("# 初始标题", cx);
        editor.set_file_path(PathBuf::from("README.md"), cx);
    });
    let multi_buffer = cx.read_entity(&editor, |editor, _| editor.multi_buffer());
    let view = cx.new(|cx| {
        MarkdownPreviewView::new(
            PreviewDocument::Source {
                path: PathBuf::from("README.md"),
                source_item: Box::new(editor.clone()),
                multi_buffer,
                open_path: None,
            },
            cx,
        )
    });
    cx.run_until_parked();
    cx.read_entity(&view, |view, _| {
        assert_eq!(
            view.blocks.as_ref(),
            &vec![Block::Heading {
                level: 1,
                content: vec![plain("初始标题")],
            }]
        );
    });

    editor.update(cx, |editor, cx| editor.set_text("更新后的正文", cx));
    cx.executor().advance_clock(MARKDOWN_REPARSE_DEBOUNCE);
    cx.run_until_parked();
    cx.read_entity(&view, |view, _| {
        assert_eq!(
            view.blocks.as_ref(),
            &vec![Block::Paragraph(vec![plain("更新后的正文")])]
        );
    });
}

#[gpui::test]
fn preview_coalesces_rapid_document_changes(cx: &mut TestAppContext) {
    let editor = cx.new(Editor::single_line);
    editor.update(cx, |editor, cx| {
        editor.set_text("初始正文", cx);
        editor.set_file_path(PathBuf::from("README.md"), cx);
    });
    let multi_buffer = cx.read_entity(&editor, |editor, _| editor.multi_buffer());
    let view = cx.new(|cx| {
        MarkdownPreviewView::new(
            PreviewDocument::Source {
                path: PathBuf::from("README.md"),
                source_item: Box::new(editor.clone()),
                multi_buffer,
                open_path: None,
            },
            cx,
        )
    });
    cx.run_until_parked();

    editor.update(cx, |editor, cx| editor.set_text("第一次修改", cx));
    editor.update(cx, |editor, cx| editor.set_text("最终内容", cx));
    cx.read_entity(&view, |view, _| {
        assert_eq!(
            view.blocks.as_ref(),
            &vec![Block::Paragraph(vec![plain("初始正文")])]
        );
    });

    cx.executor().advance_clock(MARKDOWN_REPARSE_DEBOUNCE);
    cx.run_until_parked();
    cx.read_entity(&view, |view, _| {
        assert_eq!(
            view.blocks.as_ref(),
            &vec![Block::Paragraph(vec![plain("最终内容")])]
        );
    });
}
