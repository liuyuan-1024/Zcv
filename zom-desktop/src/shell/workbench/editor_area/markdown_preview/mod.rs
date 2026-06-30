//! Markdown 预览渲染：CommonMark + GFM 子集 → GPUI 元素树。
//!
//! 架构：
//! - block 用 pulldown_cmark 事件栈构建；
//! - 行内内容用 `StyledText::with_highlights` 附加 byte-range → `HighlightStyle` 映射，一个段落只生成一个 `StyledText`；
//! - 数学公式通过 RaTeX 引擎渲染为 SVG 后嵌入。
//!
//! 子模块：
//! - `builder`: 事件流 → 中间表示（Builder 三栈状态机）
//! - `elements`: 中间表示 → GPUI 元素
//! - `math`: LaTeX → ratex → SVG → GPUI img

use std::path::Path;

use gpui::{AnyElement, ScrollHandle, div, prelude::*};
use pulldown_cmark::{Options, Parser};

use crate::theme::{color, space};

use self::builder::Builder;

mod builder;
mod elements;
mod math;

use elements::body_size;

/// 解析 markdown 源文本并渲染为 GPUI 元素树。
pub(super) fn render(
    source: &str,
    scroll_handle: Option<ScrollHandle>,
    syntax_engine: Option<&zom_workspace::syntax::SyntaxEngine>,
    base_dir: Option<&Path>,
) -> impl gpui::IntoElement {
    let blocks = parse_blocks(source, syntax_engine, base_dir, scroll_handle.clone());
    let mut container = div()
        .id("markdown-preview")
        .flex_1()
        .min_w_0()
        .w_full()
        .overflow_y_scroll()
        .p(space::s16())
        .flex()
        .flex_col()
        .gap_3()
        .bg(color::current().gray.s01)
        .text_size(body_size())
        .text_color(color::current().gray.s09);
    // 仅在有持久句柄时挂 track_scroll——句柄跨帧 / 跨 tab 切换复用，内部 Rc<RefCell<..>> 自动保持滚动位置。
    if let Some(handle) = scroll_handle {
        container = container.track_scroll(&handle);
    }
    container.children(blocks)
}

/// 解析并构建顶层 block 流。
fn parse_blocks(
    source: &str,
    syntax_engine: Option<&zom_workspace::syntax::SyntaxEngine>,
    base_dir: Option<&Path>,
    scroll_handle: Option<ScrollHandle>,
) -> Vec<AnyElement> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_MATH);

    let mut builder = Builder::new(syntax_engine, base_dir, scroll_handle);
    for event in Parser::new_ext(source, options) {
        builder.handle(event);
    }
    builder.finish()
}

// ─── 烟测 ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn block_count(source: &str) -> usize {
        parse_blocks(source, None, None, None).len()
    }

    #[test]
    fn empty_input_yields_no_blocks() {
        assert_eq!(block_count(""), 0);
    }

    #[test]
    fn unordered_list_renders_as_one_top_level_block() {
        let source = "- first\n- second\n- third\n";
        assert_eq!(block_count(source), 1);
    }

    #[test]
    fn ordered_list_renders_as_one_top_level_block() {
        let source = "1. first\n2. second\n3. third\n";
        assert_eq!(block_count(source), 1);
    }

    #[test]
    fn nested_mixed_lists_are_not_truncated() {
        let source = "- first\n- second\n  1. nested one\n  2. nested two\n- third\n";
        assert_eq!(block_count(source), 1);
    }

    #[test]
    fn code_block_with_lang_renders() {
        let source = "```rust\nlet x = 1;\n```\n";
        assert_eq!(block_count(source), 1);
    }

    #[test]
    fn table_renders() {
        let source = "| a | b |\n| --- | --- |\n| c | d |\n";
        assert_eq!(block_count(source), 1);
    }

    #[test]
    fn tight_list_with_code_block_order() {
        let source = "- 无序子项中包含：\n    ```js\n    const insideList = true;\n    ```\n";
        assert_eq!(block_count(source), 1);
    }

    // ─── 行内样式 / GFM 烟测 ─────────────────────────────────────────────────────

    #[test]
    fn bold_text_renders_one_block() {
        assert_eq!(block_count("**bold** and normal"), 1);
    }

    #[test]
    fn italic_text_renders_one_block() {
        assert_eq!(block_count("*italic* and normal"), 1);
    }

    #[test]
    fn strikethrough_renders_one_block() {
        assert_eq!(block_count("~~strikethrough~~ normal"), 1);
    }

    #[test]
    fn link_renders_one_block() {
        assert_eq!(block_count("[link](https://example.com)"), 1);
    }

    #[test]
    fn inline_code_renders_one_block() {
        assert_eq!(block_count("use `std::io` in code"), 1);
    }

    #[test]
    fn bold_italic_nested_renders_one_block() {
        assert_eq!(block_count("***bold italic*** normal"), 1);
    }

    #[test]
    fn task_list_checkbox_renders_one_block() {
        assert_eq!(block_count("- [x] done\n- [ ] pending\n"), 1);
    }

    #[test]
    fn image_alt_text_renders_one_block() {
        assert_eq!(block_count("![alt](img.png)"), 1);
    }

    #[test]
    fn thematic_break_renders() {
        // --- / *** / ___ 各自单独成行
        assert_eq!(block_count("---\n"), 1);
        assert_eq!(block_count("***\n"), 1);
        assert_eq!(block_count("___\n"), 1);
        // 带空格的写法
        assert_eq!(block_count("- - -\n"), 1);
    }

    #[test]
    fn inline_math_renders_one_block() {
        assert_eq!(block_count("$E=mc^2$"), 1);
    }

    #[test]
    fn display_math_renders_one_block() {
        assert_eq!(block_count("$$\n\\sum_{i=0}^n x_i\n$$\n"), 1);
    }

    /// 验证 pulldown-cmark 确实产出了行内公式事件。
    #[test]
    fn inline_math_event_is_produced() {
        let source = "$E=mc^2$";
        let mut options = pulldown_cmark::Options::empty();
        options.insert(pulldown_cmark::Options::ENABLE_MATH);
        let events: Vec<_> = pulldown_cmark::Parser::new_ext(source, options).collect();
        let has_math = events
            .iter()
            .any(|e| matches!(e, pulldown_cmark::Event::InlineMath(_)));
        assert!(has_math, "ENABLE_MATH 应该产出行内公式事件");
    }

    #[test]
    fn display_math_event_is_produced() {
        let source = "$$\nx\n$$";
        let mut options = pulldown_cmark::Options::empty();
        options.insert(pulldown_cmark::Options::ENABLE_MATH);
        let events: Vec<_> = pulldown_cmark::Parser::new_ext(source, options).collect();
        let has_display = events
            .iter()
            .any(|e| matches!(e, pulldown_cmark::Event::DisplayMath(_)));
        assert!(has_display, "ENABLE_MATH 应该产出块级公式事件");
    }

    /// 验证 ratex 能正确解析简单公式并生成 SVG。
    #[test]
    fn ratex_renders_simple_formula_to_svg() {
        let nodes = ratex_parser::parse("E=mc^2").expect("parse simple formula");
        let opts = ratex_layout::LayoutOptions::default();
        let lb = ratex_layout::layout(&nodes, &opts);
        let dl = ratex_layout::to_display_list(&lb);
        let svg = ratex_svg::render_to_svg(
            &dl,
            &ratex_svg::SvgOptions {
                font_size: 18.0,
                padding: 1.0,
                stroke_width: 1.2,
                embed_glyphs: true,
                font_dir: String::new(),
            },
        );
        assert!(!svg.is_empty());
        assert!(svg.contains("<svg"), "SVG 应包含 svg 标签");
    }

    // ─── flatten_highlights 测试 ──────────────────────────────────────────────────

    #[test]
    fn flatten_resolves_overlapping_ranges_last_wins() {
        use gpui::{FontWeight, HighlightStyle, Hsla};
        let text = "abcdefg";
        let style_a = HighlightStyle {
            color: Some(Hsla::default()),
            ..Default::default()
        };
        let style_b = HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        };
        // 0..6 (style_a) 与 3..7 (style_b) 在 3..6 重叠
        let highlights = vec![(0..6, style_a), (3..7, style_b)];
        let flat = elements::flatten_highlights(highlights, text);
        // 0..3 → style_a; 3..7 → style_b（重叠区后出现的胜出）
        assert_eq!(flat.len(), 2);
        assert_eq!(flat[0].0, 0..3);
        assert_eq!(flat[0].1, style_a);
        assert_eq!(flat[1].0, 3..7);
        assert_eq!(flat[1].1, style_b);
    }

    #[test]
    fn flatten_merges_adjacent_same_style() {
        use gpui::{HighlightStyle, Hsla};
        let text = "abcdef";
        let style = HighlightStyle {
            color: Some(Hsla::default()),
            ..Default::default()
        };
        // 两个相邻不重叠的同 style range → 应合并
        let highlights = vec![(0..3, style), (3..6, style)];
        let flat = elements::flatten_highlights(highlights, text);
        assert_eq!(flat.len(), 1);
        assert_eq!(flat[0].0, 0..6);
        assert_eq!(flat[0].1, style);
    }

    #[test]
    fn flatten_empty_input_is_noop() {
        let flat = elements::flatten_highlights(Vec::new(), "anything");
        assert!(flat.is_empty());
    }
}
