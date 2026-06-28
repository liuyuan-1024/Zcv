//! Markdown 元素的 GPUI 渲染实现。
//!
//! 负责将 Builder 产出的中间数据结构（BlockFrame、InlineFrame 等）转为 GPUI 元素树。

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    AnyElement, FontWeight, HighlightStyle, ImageSource, InteractiveText, ObjectFit, ScrollAnchor,
    ScrollHandle, SharedString, StyledText, div, img, prelude::*, px,
};
use pulldown_cmark::HeadingLevel;

use crate::theme::syntax as syntax_theme;
use crate::theme::{color, radius, space, typography};
use zom_workspace::syntax::SyntaxEngine;

use super::builder::{BlockFrame, BlockKind, InlineFrame, InlineKind, StyledSpan};

// ───────────────── 块构造 ─────────────────

pub(super) fn wrap_block(frame: BlockFrame) -> AnyElement {
    match frame.kind {
        BlockKind::Root => unreachable!("Root 不进入 wrap_block"),
        BlockKind::BlockQuote => div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .pl_3()
            .border_l_2()
            .border_color(color::current().gray.s04)
            .text_color(color::current().gray.s08)
            .children(frame.children)
            .into_any_element(),

        BlockKind::List { ordered_start } => {
            let mut col = div().w_full().min_w_0().flex().flex_col().gap_1().pl_4();

            let mut index = ordered_start.unwrap_or(1);

            for child in frame.children {
                let marker_text = match ordered_start {
                    Some(_) => {
                        let label = format!("{index}.");
                        index += 1;
                        label
                    }
                    None => "•".to_string(),
                };

                let marker = div()
                    .w(px(28.0))
                    .flex_none()
                    .text_color(color::current().gray.s07)
                    .font(typography::editor_font())
                    .child(SharedString::from(marker_text));

                // content 只承载一个完整的 item block，而 item block 内的段落是 StyledText。
                // 不再把 "first" / "second" 等行内文本拆成多个 div 参与 flex 测量。
                let content = div().flex_1().min_w_0().child(child);

                let row = div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_1()
                    .child(marker)
                    .child(content);

                col = col.child(row);
            }

            col.into_any_element()
        }

        BlockKind::Item => div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .children(frame.children)
            .into_any_element(),

        BlockKind::Table => div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_1()
            .py_2()
            .border_1()
            .border_color(color::current().gray.s04)
            .rounded(radius::r4())
            .children(frame.children)
            .into_any_element(),

        // pulldown_cmark 0.13 不生成 TableRow 包裹 TableHead child——TableHead 里直接就是 TableCell。
        // 所以 TableHead 和 TableRow 都要做 flex_row，TableCell 把 py_1 自带，row/head 不再多套一层 div。
        // 表格使用 flex_1 (basis:0%) 而非 flex_auto —— 只有各行的列都均分宽度，表头和表体才能列对齐。
        // min_w_0 让单元格正文拿到有限宽度，交给 StyledText 做软换行，而不是被长内容撑开。
        BlockKind::TableHead => {
            let mut row = div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_row()
                .items_start()
                .gap_2()
                .px_2()
                .py_1()
                .font_weight(FontWeight::BOLD)
                .bg(color::current().gray.s02);
            for child in frame.children {
                row = row.child(child);
            }
            row.into_any_element()
        }

        BlockKind::TableRow => {
            let mut row = div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_row()
                .items_start()
                .gap_2()
                .px_2()
                .py_1();
            for child in frame.children {
                row = row.child(child);
            }
            row.into_any_element()
        }

        BlockKind::TableCell => div()
            .flex_1()
            .min_w_0()
            .px_1()
            .children(frame.children)
            .into_any_element(),
    }
}

/// 把 markdown 链接 URL 解析为外部可打开的 URL 字符串。
///
/// - http(s):// → 原样返回
/// - # 开头 → 锚点链接，返回 None（无法外部打开）
/// - 其余 → 相对路径，基于 base_dir 转为 file:// URL
pub(super) fn resolve_link_url(raw: &str, base_dir: Option<&Path>) -> Option<String> {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return Some(raw.to_string());
    }
    if raw.starts_with('#') {
        return None;
    }
    let path = if raw.starts_with('/') {
        PathBuf::from(raw)
    } else {
        base_dir?.join(raw)
    };
    Some(format!("file://{}", path.display()))
}

/// 把标题文字转成锚点 slug：中文等非 ASCII 字符原样保留，ASCII 转小写、空格/标点替换为 `-`，连续 `-` 合并。
pub(super) fn slugify(text: &str) -> String {
    let mut slug = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if ch.is_whitespace() || ch == '-' || ch == '_' {
            slug.push('-');
        } else if !ch.is_ascii() {
            // 非 ASCII（中文等）原样保留——GitHub 风格锚点。
            slug.push(ch);
        }
        // 其余 ASCII 标点直接丢弃。
    }
    // 合并连续 `-`。
    let mut dedup = String::with_capacity(slug.len());
    let mut prev_dash = false;
    for ch in slug.chars() {
        if ch == '-' {
            if !prev_dash {
                dedup.push('-');
            }
            prev_dash = true;
        } else {
            dedup.push(ch);
            prev_dash = false;
        }
    }
    // 去掉首尾 `-`。
    dedup.trim_matches('-').to_string()
}

// ───────────────── 行内构造 ─────────────────

pub(super) fn wrap_inline(
    frame: InlineFrame,
    base_dir: Option<&Path>,
    scroll_handle: Option<&ScrollHandle>,
    heading_anchors: &Rc<RefCell<HashMap<String, ScrollAnchor>>>,
    elem_id: Option<(&'static str, u64)>,
) -> AnyElement {
    wrap_inline_impl(
        frame,
        base_dir,
        scroll_handle,
        heading_anchors,
        elem_id,
        true,
    )
}

/// `fill_width`: 为 true 时外层 div 设 w_full()（独立段落），
/// 为 false 时不设（用于 flex_row 容器内的文本段，按自然宽度参与行内流）。
pub(super) fn wrap_inline_flex(
    frame: InlineFrame,
    base_dir: Option<&Path>,
    scroll_handle: Option<&ScrollHandle>,
    heading_anchors: &Rc<RefCell<HashMap<String, ScrollAnchor>>>,
    elem_id: Option<(&'static str, u64)>,
) -> AnyElement {
    wrap_inline_impl(
        frame,
        base_dir,
        scroll_handle,
        heading_anchors,
        elem_id,
        false,
    )
}

fn wrap_inline_impl(
    frame: InlineFrame,
    base_dir: Option<&Path>,
    scroll_handle: Option<&ScrollHandle>,
    heading_anchors: &Rc<RefCell<HashMap<String, ScrollAnchor>>>,
    elem_id: Option<(&'static str, u64)>,
    fill_width: bool,
) -> AnyElement {
    let (full_text, highlights) = build_highlighted_text(&frame.spans);
    let highlights = flatten_highlights(highlights, &full_text);
    let styled = StyledText::new(full_text).with_highlights(highlights);

    // 将 link_spans 转为 InteractiveText::on_click 的 (range, url) 对。
    let link_ranges: Vec<(std::ops::Range<usize>, String)> = frame
        .link_spans
        .iter()
        .filter(|ls| ls.end > ls.start)
        .map(|ls| (ls.start..ls.end, ls.url.clone()))
        .collect();

    let heading_anchors_for_click = Rc::clone(heading_anchors);

    let text_element: AnyElement = if link_ranges.is_empty() {
        styled.into_any_element()
    } else {
        let ranges: Vec<std::ops::Range<usize>> =
            link_ranges.iter().map(|(r, _)| r.clone()).collect();
        // 预先解析 URL：http(s) → 原样，file → file://，anchor → "#…" 标记。
        let urls: Vec<Option<String>> = link_ranges
            .iter()
            .map(|(_, raw)| resolve_link_url(raw, base_dir))
            .collect();
        let anchors: Vec<Option<String>> = link_ranges
            .iter()
            .map(|(_, raw)| {
                if raw.starts_with('#') {
                    Some(raw[1..].to_string())
                } else {
                    None
                }
            })
            .collect();
        let text_id = "md-text";
        InteractiveText::new(text_id, styled)
            .on_click(ranges, move |idx, _window, app| {
                // 锚点链接优先：滚动到目标标题。
                if let Some(Some(anchor)) = anchors.get(idx) {
                    let map = heading_anchors_for_click.borrow();
                    if let Some(scroll_anchor) = map.get(anchor) {
                        scroll_anchor.scroll_to(_window, app);
                        return;
                    }
                }
                // http(s) / file:// / 相对路径 → 全部走 resolve_link_url 处理后 open_url。
                if let Some(Some(url)) = urls.get(idx) {
                    app.open_url(url);
                }
            })
            .into_any_element()
    };

    let mut base = div()
        .min_w_0()
        .whitespace_normal()
        .line_height(typography::editor_line())
        .child(text_element);
    if fill_width {
        base = base.w_full();
    }

    match frame.kind {
        InlineKind::Paragraph => base.into_any_element(),
        InlineKind::Heading(level) => {
            let heading_text: String = frame
                .spans
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>();
            let slug = slugify(&heading_text);

            if let (Some(handle), Some(id), false) = (scroll_handle, elem_id, slug.is_empty()) {
                let anchor = ScrollAnchor::for_handle(handle.clone());
                heading_anchors.borrow_mut().insert(slug, anchor.clone());
                base.id(id)
                    .text_size(heading_size(level))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color::current().gray.s09)
                    .anchor_scroll(Some(anchor))
                    .into_any_element()
            } else {
                base.text_size(heading_size(level))
                    .font_weight(FontWeight::BOLD)
                    .text_color(color::current().gray.s09)
                    .into_any_element()
            }
        }
    }
}

// ───────────────── 辅助：文本高亮构建 ─────────────────

/// 将 span 列表拼接为完整文本 + byte-range → HighlightStyle 映射。
pub(super) fn build_highlighted_text(
    spans: &[StyledSpan],
) -> (String, Vec<(std::ops::Range<usize>, HighlightStyle)>) {
    let mut text = String::new();
    let mut highlights: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();

    for span in spans {
        let start = text.len();
        text.push_str(&span.text);
        let end = text.len();
        if span.style != HighlightStyle::default() {
            highlights.push((start..end, span.style));
        }
    }

    (text, highlights)
}

/// 把可能重叠的 highlight range 展开为不重叠的相邻段。
///
/// tree-sitter 对同一段文本可能产出嵌套 capture（如 `string` 里嵌套 `string.special`）。
/// GPUI `compute_runs` 假设 range 不重叠且按 start 有序——重叠会导致内部字节指针倒退、TextRun 总长度错位，最终触发 text_system panic。
///
/// 展平后：重叠区域以后出现的 range 的 style 为准；相邻同 style 段自动合并。
///
/// **前置条件**：所有 range 的 start/end 必须落在 UTF-8 char boundary 上。
pub(super) fn flatten_highlights(
    highlights: Vec<(std::ops::Range<usize>, HighlightStyle)>,
    text: &str,
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    debug_assert!(
        highlights
            .iter()
            .all(|(r, _)| { text.is_char_boundary(r.start) && text.is_char_boundary(r.end) }),
        "flatten_highlights: 所有 range 端点必须在 char boundary 上"
    );

    if highlights.is_empty() {
        return highlights;
    }

    // 收集所有端点并排序去重
    let mut points: Vec<usize> = Vec::with_capacity(highlights.len() * 2);
    for (range, _) in &highlights {
        points.push(range.start);
        points.push(range.end);
    }
    points.sort();
    points.dedup();

    // 逐段确定 style（后出现的 range 胜出），合并相邻同 style 段
    let mut flat: Vec<(std::ops::Range<usize>, HighlightStyle)> = Vec::new();
    for window in points.windows(2) {
        let seg_start = window[0];
        let seg_end = window[1];
        let style = highlights
            .iter()
            .rev()
            .find(|(range, _)| range.start <= seg_start && range.end >= seg_end)
            .map(|(_, style)| *style);

        if let Some(style) = style {
            match flat.last_mut() {
                Some((last_range, last_style))
                    if *last_style == style && last_range.end == seg_start =>
                {
                    last_range.end = seg_end;
                }
                _ => flat.push((seg_start..seg_end, style)),
            }
        }
    }

    flat
}

// ───────────────── 代码块 ─────────────────

pub(super) fn code_block_element(
    text: &str,
    lang: Option<&str>,
    syntax_engine: Option<&SyntaxEngine>,
) -> AnyElement {
    let mut block = div()
        .p(space::s8())
        .rounded(radius::r4())
        .bg(color::current().gray.s02)
        .text_color(color::current().gray.s09)
        .font(typography::editor_font())
        .text_size(typography::editor())
        .line_height(typography::editor_line());

    if let Some(lang) = lang {
        block = block.child(
            div()
                .text_size(typography::ui())
                .text_color(color::current().gray.s07)
                .mb_1()
                .child(SharedString::from(lang.to_string())),
        );
    }

    let trimmed = text.trim_end_matches('\n');
    if trimmed.is_empty() {
        return block.into_any_element();
    }

    // 尝试语法高亮：SyntaxEngine 内聚了别名归一化 + provider 查找 + 一次性 highlight query。
    if let (Some(lang), Some(engine)) = (lang, syntax_engine) {
        if let Some(spans) = engine.highlight_snippet(lang, trimmed) {
            let highlights: Vec<_> = spans
                .into_iter()
                .map(|(range, name)| {
                    (
                        range,
                        HighlightStyle {
                            color: Some(syntax_theme::color_for(name.as_str())),
                            ..Default::default()
                        },
                    )
                })
                .collect();
            let text = trimmed.to_string();
            let highlights = flatten_highlights(highlights, &text);
            return block
                .child(StyledText::new(text).with_highlights(highlights))
                .into_any_element();
        }
    }

    // 退化路径：无语言标签 / 无引擎 / 不支持的语言 → 逐行纯文本。
    for line in trimmed.lines() {
        block = block.child(SharedString::from(line.to_string()));
    }

    block.into_any_element()
}

// ───────────────── 图片 ─────────────────

/// 把 markdown 里的图片 URL 解析为 GPUI `ImageSource`。
///
/// - http(s):// → `Resource::Uri`
/// - / 开头 → 文件系统绝对路径 → `Resource::Path`
/// - 其余 → 相对于 base_dir 解析 → `Resource::Path`
fn resolve_image_source(dest_url: &str, base_dir: Option<&Path>) -> ImageSource {
    if dest_url.starts_with("http://") || dest_url.starts_with("https://") {
        return ImageSource::from(dest_url.to_string());
    }
    let path = if dest_url.starts_with('/') {
        PathBuf::from(dest_url)
    } else {
        match base_dir {
            Some(base) => base.join(dest_url),
            None => PathBuf::from(dest_url),
        }
    };
    ImageSource::from(path)
}

pub(super) fn image_element(
    dest_url: &str,
    alt: &str,
    title: &str,
    base_dir: Option<&Path>,
    id: (&'static str, u64),
) -> AnyElement {
    let source = resolve_image_source(dest_url, base_dir);
    let fallback_text = if alt.is_empty() { "(图片)" } else { alt };
    let fallback = {
        let fb = fallback_text.to_string();
        move || {
            div()
                .w_full()
                .py_2()
                .px_3()
                .bg(color::current().gray.s02)
                .border_1()
                .border_color(color::current().gray.s04)
                .rounded(radius::r4())
                .text_color(color::current().gray.s07)
                .text_size(typography::ui())
                .child(SharedString::from(format!("🖼 {fb}")))
                .into_any_element()
        }
    };
    // .id(…) 将 Img 转为 Stateful<Img>，使异步加载状态能跨帧持久化。
    let img_el = img(source)
        .id(id)
        .object_fit(ObjectFit::ScaleDown)
        .w_full()
        .min_h(px(100.0))
        .max_h(px(400.0))
        .rounded(radius::r4())
        .with_fallback(fallback);
    let mut wrapper = div().w_full().min_w_0().py_2().child(img_el);
    // 有 title 时在其下方附加小字
    if !title.is_empty() {
        wrapper = wrapper.child(
            div()
                .w_full()
                .text_size(typography::ui())
                .text_color(color::current().gray.s06)
                .mt_1()
                .child(SharedString::from(title.to_string())),
        );
    }
    wrapper.into_any_element()
}

// ───────────────── 水平线 ─────────────────

pub(super) fn horizontal_rule() -> AnyElement {
    div()
        .h(px(1.0))
        .w_full()
        .flex_none()
        .my_2()
        .bg(color::current().gray.s04)
        .into_any_element()
}

// ───────────────── 视觉常量 ─────────────────

pub(super) fn body_size() -> gpui::Pixels {
    typography::editor()
}

pub(super) fn heading_size(level: HeadingLevel) -> gpui::Pixels {
    match level {
        HeadingLevel::H1 => px(26.0),
        HeadingLevel::H2 => px(22.0),
        HeadingLevel::H3 => px(19.0),
        HeadingLevel::H4 => px(17.0),
        HeadingLevel::H5 | HeadingLevel::H6 => px(16.0),
    }
}
