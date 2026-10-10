use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AppContext, Context, IntoElement, Modifiers, ParentElement, Render, StyledText, TestAppContext,
    TextAlign, TextLayout, Window, div, point, px, size,
};
use zcv_editor::Editor;
use zcv_theme::typography;
use zcv_workspace::PreviewDocument;

use crate::document::{Inline, InlineStyle, parse};

use super::{
    Block, InlineText, MARKDOWN_REPARSE_DEBOUNCE, MATH_PLACEHOLDER, MarkdownInlineText,
    MarkdownPreviewView, MarkdownRenderContext, MathImage, MathImages, MathPlacement,
    OpenPathCallback, code_lines, heading_line_height, heading_size, highlight_code_blocks,
    inline_math_bounds, list_marker_char_count, render_block, render_math, render_text_inline,
    resolve_markdown_file_link, visible_highlights_for_line,
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
            math_placements: Vec::new(),
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
            large.image.size(0).width > small.image.size(0).width,
            "公式图像应随内容字号放大"
        );
        assert!(
            large.image.size(0).height > small.image.size(0).height,
            "公式图像高度应随内容字号放大"
        );
        assert!(
            large.size.width > small.size.width && large.size.height > small.size.height,
            "公式逻辑尺寸应随内容字号放大"
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
    let editor = cx.new(|cx| Editor::single_line(Arc::new(LanguageRegistry::new()), cx));
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
    let editor = cx.new(|cx| Editor::single_line(Arc::new(LanguageRegistry::new()), cx));
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

/// 预览工具区由工作区的 PreviewToolbar 承担；
/// 预览视图通过 `act_as_type` 暴露源码编辑器，但自身不是编辑器 Item，
/// 工具项注册方据此隐藏通用文档工具栏，两行工具区不会重复显示。
#[gpui::test]
fn preview_exposes_the_source_editor_as_a_proxy(cx: &mut TestAppContext) {
    let editor = cx.new(|cx| Editor::single_line(Arc::new(LanguageRegistry::new()), cx));
    editor.update(cx, |editor, cx| {
        editor.set_text("# 标题", cx);
        editor.set_file_path(PathBuf::from("README.md"), cx);
    });
    let multi_buffer = cx.read_entity(&editor, |editor, _| editor.multi_buffer());
    let view = cx.new(|cx| {
        MarkdownPreviewView::new(
            PreviewDocument::Source {
                path: PathBuf::from("README.md"),
                source_item: Box::new(editor),
                multi_buffer,
                open_path: None,
            },
            cx,
        )
    });
    let item_id = view.entity_id();
    let handle: Box<dyn zcv_workspace::ItemHandle> = Box::new(view);
    cx.read(|cx| {
        let exposed = handle
            .act_as::<Editor>(cx)
            .expect("预览仍应向工作区暴露源码编辑器");
        assert_ne!(
            exposed.entity_id(),
            item_id,
            "预览暴露的是源码编辑器，自身不是编辑器 Item"
        );
    });
}
fn math_inline(source: &str) -> Inline {
    Inline {
        text: source.into(),
        style: InlineStyle {
            math: true,
            ..InlineStyle::default()
        },
    }
}

fn test_render_image(app: &mut gpui::App) -> Arc<gpui::RenderImage> {
    app.svg_renderer()
        .render_single_frame(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"></svg>"#,
            1.0,
        )
        .expect("测试 SVG 应能渲染")
}

#[gpui::test]
fn inline_math_reserves_width_in_the_text_flow(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    cx.update(|window, app| {
        let type_scale = typography::current(app);
        let math_size = size(px(40.), px(20.));
        let mut math_images = MathImages::new();
        math_images.insert(
            "x^2".to_owned(),
            Ok(MathImage {
                image: test_render_image(app),
                size: math_size,
            }),
        );
        let render_context = MarkdownRenderContext {
            source_directory: None,
            open_path: None,
            type_scale,
            math_images: &math_images,
            window,
            cx: app,
        };

        let prefix = "前前前";
        let suffix = "后后后";
        let content = vec![plain(prefix), math_inline("x^2"), plain(suffix)];
        let mut inline_text = InlineText::default();
        for inline in &content {
            if inline.style.math {
                inline_text.push_math(&inline.text, &render_context);
            } else {
                inline_text.push(&inline.text, &inline.style, app);
            }
        }

        let placeholder_bytes = MATH_PLACEHOLDER.len_utf8();
        let placeholder_count = inline_text.text.matches(MATH_PLACEHOLDER).count();
        assert!(placeholder_count >= 1, "公式应在文本流中预留占位宽度");

        let [placement] = inline_text.math_placements.as_slice() else {
            panic!("应记录一个行内公式位置");
        };
        assert_eq!(placement.byte_offset, prefix.len());
        assert_eq!(placement.size, math_size);

        let suffix_start = placement.byte_offset + placeholder_count * placeholder_bytes;
        assert_eq!(&inline_text.text[..placement.byte_offset], prefix);
        assert_eq!(&inline_text.text[suffix_start..], suffix);

        let space = {
            let font_id = window
                .text_system()
                .resolve_font(&window.text_style().font());
            window
                .text_system()
                .layout_width(font_id, type_scale.content_size(), MATH_PLACEHOLDER)
        };
        let reserved = space * placeholder_count as f32;
        assert_eq!(placement.reserved, reserved, "预留宽度应与占位字符数一致");
        assert!(
            reserved >= math_size.width,
            "占位宽度应覆盖公式宽度：reserved={reserved:?}"
        );
        assert!(
            reserved - math_size.width < space,
            "占位宽度不应比公式多出一个占位字符"
        );

        let runs = vec![window.text_style().to_run(inline_text.text.len())];
        let wrapped = window
            .text_system()
            .shape_text(
                inline_text.text.clone().into(),
                type_scale.content_size(),
                &runs,
                Some(px(60.)),
                None,
            )
            .expect("文本应能排版");
        for line in &wrapped {
            for boundary in line.wrap_boundaries() {
                let run = &line.unwrapped_layout.runs[boundary.run_ix];
                let index = run.glyphs[boundary.glyph_ix].index;
                assert!(
                    !(placement.byte_offset < index && index < suffix_start),
                    "换行点不得落在公式占位区域内部：index={index}"
                );
            }
        }
    });
}

#[gpui::test]
fn list_marker_height_matches_the_first_line_of_a_wrapped_item(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let source = "1. 前前前前前前前前前前前前前前前前前前前前 $x^2$ 后后后后后后后后后后后后后后后后后后后后";
    let blocks = parse(source);
    let math_height = px(80.);
    cx.draw(
        Default::default(),
        size(px(400.), px(400.)),
        move |window, app| {
            let type_scale = typography::current(app);
            let mut math_images = MathImages::new();
            math_images.insert(
                "x^2".to_owned(),
                Ok(MathImage {
                    image: test_render_image(app),
                    size: size(px(60.), math_height),
                }),
            );
            let render_context = MarkdownRenderContext {
                source_directory: None,
                open_path: None,
                type_scale,
                math_images: &math_images,
                window,
                cx: app,
            };
            let mut next_key = 0;
            render_block(&blocks[0], &mut next_key, 0, 0, &render_context)
        },
    );

    let marker = cx
        .debug_bounds("markdown-list-marker")
        .expect("应渲染列表项头");
    let content = cx
        .debug_bounds("markdown-list-item-content")
        .expect("应渲染列表项内容");
    let expected_first_line =
        cx.update(|_, app| typography::current(app).content_line().max(math_height));
    assert!(
        content.size.height > expected_first_line,
        "用例应让列表项内容换行成多行：content={:?}",
        content.size
    );
    assert_eq!(
        marker.size.height, expected_first_line,
        "项头只应占首行高度；按整项高度拉伸会让项头错误地垂直居中"
    );
    assert_eq!(marker.origin.y, px(0.), "项头应顶端对齐首行");
}
/// 绘制测试中捕获的段落布局与行内公式位置。
type CapturedInlineLayout = Rc<RefCell<Option<(TextLayout, Vec<MathPlacement>)>>>;

#[gpui::test]
fn inline_math_paints_on_the_wrapped_line_where_the_placeholder_starts(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();
    let source = "例如，一个点对的距离和三个角度分别为 $(44\\text{ mm},32^\\circ,67^\\circ,114^\\circ)$，量化后得到键 $(5,1,3,6)$。另一个点对的数值如果略有不同，但仍落在这四个相同的格子里，也会得到同一个键，因而可以通过哈希表互相查到。";
    let blocks = parse(source);
    let Block::Paragraph(content) = &blocks[0] else {
        panic!("应解析为段落");
    };
    let content = content.clone();

    let captured: CapturedInlineLayout = Rc::new(RefCell::new(None));
    let captured_in_draw = captured.clone();
    cx.draw(
        Default::default(),
        size(px(560.), px(400.)),
        move |window, app| {
            let type_scale = typography::current(app);
            let mut math_images = MathImages::new();
            for (key, width) in [
                ("(44\\text{ mm},32^\\circ,67^\\circ,114^\\circ)", 220.),
                ("(5,1,3,6)", 100.),
            ] {
                math_images.insert(
                    key.to_owned(),
                    Ok(MathImage {
                        image: test_render_image(app),
                        size: size(px(width), px(24.)),
                    }),
                );
            }
            let render_context = MarkdownRenderContext {
                source_directory: None,
                open_path: None,
                type_scale,
                math_images: &math_images,
                window,
                cx: app,
            };
            let mut inline_text = InlineText::default();
            for inline in &content {
                if inline.style.math {
                    inline_text.push_math(&inline.text, &render_context);
                } else {
                    inline_text.push(&inline.text, &inline.style, app);
                }
            }
            let text = StyledText::new(inline_text.text).with_highlights(inline_text.highlights);
            let layout = text.layout().clone();
            *captured_in_draw.borrow_mut() =
                Some((layout.clone(), inline_text.math_placements.clone()));
            MarkdownInlineText {
                text: text.into_any_element(),
                layout,
                code_ranges: Vec::new(),
                code_color: gpui::black(),
                math_placements: inline_text.math_placements,
            }
        },
    );

    let captured = captured.borrow();
    let (layout, placements) = captured.as_ref().expect("应捕获段落布局");
    assert_eq!(placements.len(), 2, "用例应包含两个行内公式");

    let boundaries: Vec<usize> = layout
        .line_layouts()
        .iter()
        .flat_map(|line| {
            let unwrapped = &line.unwrapped_layout;
            line.wrap_boundaries().iter().map(move |boundary| {
                unwrapped.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index
            })
        })
        .collect();
    assert!(
        boundaries.contains(&placements[1].byte_offset),
        "用例应让第二个公式恰好从换行边界开始：boundaries={boundaries:?} offset={}",
        placements[1].byte_offset
    );

    let first = inline_math_bounds(layout, &placements[0], TextAlign::Left)
        .expect("第一个公式应有绘制位置");
    let second = inline_math_bounds(layout, &placements[1], TextAlign::Left)
        .expect("第二个公式应有绘制位置");
    let line_top = layout.bounds().origin.y;
    let line_height = layout.line_height();
    assert!(
        first.origin.y < line_top + line_height,
        "第一个公式应画在首行：first={:?}",
        first.origin
    );
    assert_eq!(
        second.origin.y - first.origin.y,
        line_height,
        "占位符从换行边界开始时，公式应画在新行而不是上一行末尾"
    );
}
