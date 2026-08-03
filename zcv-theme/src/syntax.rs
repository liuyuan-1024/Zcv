//! 语法高亮：tree-sitter capture name → GPUI HighlightStyle。
//!
//! 本模块只提供查询机制，不定义色值。色值来自 vendor 的主题文件（`assets/themes/*.toml`）。
//! 查询走点分前缀回退：`keyword.control.import` 未命中 → `keyword.control` → `keyword` → [`default_fg`]。

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use gpui::{
    FontStyle, FontWeight, HighlightStyle, Hsla, Rgba, StrikethroughStyle, UnderlineStyle, px, rgb,
    rgba,
};

use super::color;
use crate::Theme;

const THEME_ONE_DARK_TOML: &str = include_str!("../assets/themes/onedark.toml");
const THEME_ONE_LIGHT_TOML: &str = include_str!("../assets/themes/onelight.toml");

pub fn default_fg(cx: &gpui::App) -> Hsla {
    color::current(cx).text.into()
}

/// 按 highlight name 解析字色，走点分前缀回退链。
pub fn color_for(name: &str, cx: &gpui::App) -> Hsla {
    style_for(name).color.unwrap_or_else(|| default_fg(cx))
}

/// 预展开 capture 名字表为按索引直接取用的样式表。
///
/// 每个名字做一次点分前缀回退；
/// 渲染侧按 capture index 一次数组索引，不再逐 run 做字符串查找与回退。
pub fn style_table(names: &[Arc<str>]) -> Vec<HighlightStyle> {
    names.iter().map(|name| style_for(name)).collect()
}

/// 按 capture name 解析完整样式。
pub fn style_for(name: &str) -> HighlightStyle {
    let mut current = name;
    loop {
        if let Some(style) = lookup_in_theme(current) {
            return style;
        }
        match current.rfind('.') {
            Some(dot) => current = &current[..dot],
            None => return HighlightStyle::default(),
        }
    }
}

pub(crate) fn set_theme(theme: Theme) {
    let source = match theme {
        Theme::OneDark => THEME_ONE_DARK_TOML,
        Theme::OneLight => THEME_ONE_LIGHT_TOML,
        // System 未解析时按深色默认。
        Theme::System => THEME_ONE_DARK_TOML,
    };
    let table = parse_helix_theme(source).unwrap_or_default();
    let lock = ACTIVE_THEME.get_or_init(|| RwLock::new(default_theme_table()));
    match lock.write() {
        Ok(mut active) => *active = table,
        Err(error) => eprintln!("更新语法主题失败：{error}"),
    }
}

fn lookup_in_theme(name: &str) -> Option<HighlightStyle> {
    let lock = ACTIVE_THEME.get_or_init(|| RwLock::new(default_theme_table()));
    lock.read().ok().and_then(|theme| theme.get(name).copied())
}

/// 解析后主题表。解析失败则空表，所有 name 落 default_fg。
static ACTIVE_THEME: OnceLock<RwLock<HashMap<&'static str, HighlightStyle>>> = OnceLock::new();

fn default_theme_table() -> HashMap<&'static str, HighlightStyle> {
    parse_helix_theme(THEME_ONE_DARK_TOML).unwrap_or_default()
}

/// 解析 Helix 风格 theme.toml 为 name → Hsla 表。
///
/// 支持 `"name" = "color"` 和 `"name" = { fg = "color" }` 两种格式。
/// 不含 `fg` 的条目跳过，`[palette]` 用于解析颜色名引用。
/// 解析失败不阻断渲染，所有未命中 name 落 default_fg。
fn parse_helix_theme(src: &str) -> Option<HashMap<&'static str, HighlightStyle>> {
    let root: toml::Table = toml::from_str(src).ok()?;

    let palette: HashMap<String, Rgba> = root
        .get("palette")
        .and_then(|v| v.as_table())
        .map(|tbl| {
            tbl.iter()
                .filter_map(|(k, v)| {
                    let hex = v.as_str()?;
                    let color = parse_hex(hex)?;
                    Some((k.clone(), color))
                })
                .collect()
        })
        .unwrap_or_default();

    let resolve = |color: &str| -> Option<Rgba> {
        if color.starts_with('#') {
            parse_hex(color)
        } else {
            palette.get(color).copied()
        }
    };

    let mut out: HashMap<&'static str, HighlightStyle> = HashMap::new();
    for (key, value) in &root {
        if key == "palette" {
            continue;
        }
        let (color_token, modifiers): (Option<&str>, &[toml::Value]) = match value {
            toml::Value::String(s) => (Some(s.as_str()), &[]),
            toml::Value::Table(t) => (
                t.get("fg").and_then(|v| v.as_str()),
                t.get("modifiers")
                    .and_then(|v| v.as_array())
                    .map_or(&[], Vec::as_slice),
            ),
            _ => (None, &[]),
        };
        let color = color_token.and_then(resolve).map(Hsla::from);
        if color_token.is_some() && color.is_none() {
            continue;
        }
        let has_modifier = |name: &str| modifiers.iter().any(|value| value.as_str() == Some(name));
        let style = HighlightStyle {
            color,
            font_weight: has_modifier("bold").then_some(FontWeight::BOLD),
            font_style: has_modifier("italic").then_some(FontStyle::Italic),
            underline: has_modifier("underlined").then_some(UnderlineStyle {
                thickness: px(1.),
                color,
                wavy: false,
            }),
            strikethrough: has_modifier("crossed_out").then_some(StrikethroughStyle {
                thickness: px(1.),
                color,
            }),
            ..HighlightStyle::default()
        };
        if style == HighlightStyle::default() {
            continue;
        }
        let static_key: &'static str = Box::leak(key.clone().into_boxed_str());
        out.insert(static_key, style);
    }
    Some(out)
}

fn parse_hex(s: &str) -> Option<Rgba> {
    let body = s.strip_prefix('#')?;
    let value = u32::from_str_radix(body, 16).ok()?;
    Some(match body.len() {
        6 => rgb(value),
        8 => rgba(value),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_chain_terminates_at_default_style() {
        assert_eq!(style_for("totally.unknown"), HighlightStyle::default());
        assert_eq!(style_for("nope"), HighlightStyle::default());
    }

    #[test]
    fn onedark_provides_color_for_common_rust_names() {
        for name in &["keyword", "string", "comment", "function", "type"] {
            assert!(
                style_for(name).color.is_some(),
                "onedark 必须给 `{name}` 上色"
            );
        }
    }

    #[test]
    fn dot_prefix_fallback_uses_parent_rule() {
        assert_eq!(
            style_for("function.method").color,
            style_for("function").color
        );
        assert_eq!(style_for("type.builtin").color, style_for("type").color);
        assert_eq!(
            style_for("comment.documentation").color,
            style_for("comment").color
        );
    }

    #[test]
    fn parser_resolves_palette_references() {
        let src = r##"
            "keyword" = { fg = "red" }
            "string" = "green"
            "comment" = "#abcdef"
            [palette]
            red = "#ff0000"
            green = "#00ff00"
        "##;
        let table = parse_helix_theme(src).expect("应能解析主题");
        assert_eq!(
            table.get("keyword").unwrap().color,
            Some(Hsla::from(rgb(0xff0000)))
        );
        assert_eq!(
            table.get("string").unwrap().color,
            Some(Hsla::from(rgb(0x00ff00)))
        );
        assert_eq!(
            table.get("comment").unwrap().color,
            Some(Hsla::from(rgb(0xabcdef)))
        );
    }

    #[test]
    fn parser_skips_entries_without_fg() {
        let src = r##"
            "ui.background" = { bg = "black" }
            "diagnostic.unnecessary" = { modifiers = ["dim"] }
            "keyword" = { fg = "red" }
            [palette]
            red = "#ff0000"
            black = "#000000"
        "##;
        let table = parse_helix_theme(src).expect("应能解析主题");
        assert!(!table.contains_key("ui.background"));
        assert!(!table.contains_key("diagnostic.unnecessary"));
        assert!(table.contains_key("keyword"));
    }

    #[test]
    fn parser_skips_unresolved_color_tokens() {
        let src = r##"
            "keyword" = "missing-color"
            "string" = "red"
            [palette]
            red = "#ff0000"
        "##;
        let table = parse_helix_theme(src).expect("应能解析主题");
        assert!(!table.contains_key("keyword"));
        assert!(table.contains_key("string"));
    }

    #[test]
    fn parser_handles_invalid_toml() {
        assert!(parse_helix_theme("this is not [valid toml").is_none());
    }

    #[test]
    fn lsp_parameter_resolves_to_color_via_variable_parameter() {
        assert!(style_for("variable.parameter").color.is_some());
    }

    #[test]
    fn lsp_method_falls_back_to_function() {
        assert_eq!(
            style_for("function.method").color,
            style_for("function").color
        );
    }

    #[test]
    fn lsp_enum_member_resolves_via_variable_other_member() {
        assert!(style_for("variable.other.member").color.is_some());
    }

    #[test]
    fn lsp_macro_resolves_via_function_dot_macro() {
        assert!(style_for("function.macro").color.is_some());
    }

    #[test]
    fn markdown_capture_rules_keep_theme_modifiers() {
        assert_eq!(style_for("text.strong").font_weight, Some(FontWeight::BOLD));
        assert_eq!(
            style_for("text.emphasis").font_style,
            Some(FontStyle::Italic)
        );
        assert!(style_for("text.title").color.is_some());
    }
}
