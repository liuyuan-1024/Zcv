//! 数学公式渲染：LaTeX → ratex → SVG → GPUI img 元素。
//!
//! 使用 RaTeX 纯 Rust KaTeX 兼容引擎。
//! SVG 写入临时文件后通过 gpui ImageSource::Resource 渲染——GPUI 内置按 .svg 扩展名识别并调用 resvg/usvg 光栅化。

use gpui::{AnyElement, ImageSource, img, prelude::*};

use crate::theme;

/// 将 LaTeX 数学源码渲染为 GPUI 元素。
/// 行内和块级公式目前统一按块级渲染（GPUI 不支持 inline 布局）。
pub(super) fn math_element(latex: &str, display_mode: bool) -> AnyElement {
    let text_color = theme::color::current().gray.s09;
    let ratex_color =
        ratex_types::color::Color::new(text_color.r, text_color.g, text_color.b, text_color.a);

    match render_latex_to_svg(latex, display_mode, ratex_color) {
        Ok(svg_str) => {
            let path = svg_to_tempfile(&svg_str);
            // 不设 w_full()：让 img 以 SVG 的自然尺寸显示，避免被拉伸。
            img(ImageSource::from(path)).into_any_element()
        }
        Err(_) => fallback_text(latex),
    }
}

/// 将 SVG 写入临时文件。按内容哈希命名保证不同公式各自独立。
fn svg_to_tempfile(svg: &str) -> std::path::PathBuf {
    use std::hash::{Hash, Hasher};
    use std::io::Write;

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    svg.hash(&mut hasher);
    let hash = hasher.finish();

    let mut path = std::env::temp_dir();
    path.push(format!("zom-math-{hash:x}.svg"));
    if let Ok(mut f) = std::fs::File::create(&path) {
        let _ = f.write_all(svg.as_bytes());
    }
    path
}

/// LaTeX → SVG 字符串。
fn render_latex_to_svg(
    latex: &str,
    display_mode: bool,
    color: ratex_types::color::Color,
) -> Result<String, String> {
    // 块级公式的 $$ 定界符内可能包含前导/尾随换行。
    let latex = latex.trim();
    let nodes = ratex_parser::parse(latex).map_err(|e| format!("公式解析失败: {e}"))?;

    let style = if display_mode {
        ratex_types::math_style::MathStyle::Display
    } else {
        ratex_types::math_style::MathStyle::Text
    };

    let layout_options = ratex_layout::LayoutOptions::default()
        .with_style(style)
        .with_color(color);

    let layout_box = ratex_layout::layout(&nodes, &layout_options);
    let display_list = ratex_layout::to_display_list(&layout_box);

    // 跟随编辑器字号：行内公式与正文同大，块级公式略大。
    let body_size = theme::typography::editor_font_size() as f64;
    let font_size = if display_mode {
        body_size * 1.25
    } else {
        body_size
    };

    let svg = ratex_svg::render_to_svg(
        &display_list,
        &ratex_svg::SvgOptions {
            font_size,
            padding: 0.0,
            stroke_width: body_size * 0.07,
            embed_glyphs: true,
            font_dir: String::new(),
        },
    );

    Ok(svg)
}

/// 渲染失败退化展示。
fn fallback_text(latex: &str) -> AnyElement {
    use theme::{color, radius, space, typography};
    gpui::div()
        .px(space::s4())
        .py(gpui::px(1.0))
        .rounded(radius::r4())
        .bg(color::current().gray.s02)
        .text_color(color::current().gray.s07)
        .font(typography::editor_font())
        .text_size(typography::editor())
        .child(format!("${latex}$"))
        .into_any_element()
}
