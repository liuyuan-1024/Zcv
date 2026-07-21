//! 语法高亮：tree-sitter highlight name → 字色。
//!
//! 本模块只提供查询机制，不定义色值。色值来自 vendor 的主题文件（`assets/themes/*.toml`）。
//! 查询走点分前缀回退：`keyword.control.import` 未命中 → `keyword.control` → `keyword` → [`default_fg`]。

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use gpui::{Hsla, Rgba, rgb, rgba};

use super::color;
use crate::theme::ConcreteTheme;

const THEME_ONE_DARK_TOML: &str = include_str!("../../assets/themes/onedark.toml");
const THEME_ONE_LIGHT_TOML: &str = include_str!("../../assets/themes/onelight.toml");

pub fn default_fg() -> Hsla {
    color::current().gray.s[8].into()
}

/// 按 highlight name 解析字色，走点分前缀回退链。
pub fn color_for(name: &str) -> Hsla {
    let mut current = name;
    loop {
        if let Some(hsla) = lookup_in_theme(current) {
            return hsla;
        }
        match current.rfind('.') {
            Some(dot) => current = &current[..dot],
            None => return default_fg(),
        }
    }
}

pub(crate) fn set_theme(theme: ConcreteTheme) {
    let source = match theme {
        ConcreteTheme::Dark => THEME_ONE_DARK_TOML,
        ConcreteTheme::Light => THEME_ONE_LIGHT_TOML,
    };
    let table = parse_helix_theme(source).unwrap_or_default();
    let lock = ACTIVE_THEME.get_or_init(|| RwLock::new(default_theme_table()));
    match lock.write() {
        Ok(mut active) => *active = table,
        Err(error) => eprintln!("更新语法主题失败：{error}"),
    }
}

fn lookup_in_theme(name: &str) -> Option<Hsla> {
    let lock = ACTIVE_THEME.get_or_init(|| RwLock::new(default_theme_table()));
    lock.read().ok().and_then(|theme| theme.get(name).copied())
}

/// 解析后主题表。解析失败则空表，所有 name 落 default_fg。
static ACTIVE_THEME: OnceLock<RwLock<HashMap<&'static str, Hsla>>> = OnceLock::new();

fn default_theme_table() -> HashMap<&'static str, Hsla> {
    parse_helix_theme(THEME_ONE_DARK_TOML).unwrap_or_default()
}

/// 解析 Helix 风格 theme.toml 为 name → Hsla 表。
///
/// 支持 `"name" = "color"` 和 `"name" = { fg = "color" }` 两种格式。
/// 不含 `fg` 的条目跳过，`[palette]` 用于解析颜色名引用。
/// 解析失败不阻断渲染，所有未命中 name 落 default_fg。
fn parse_helix_theme(src: &str) -> Option<HashMap<&'static str, Hsla>> {
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

    let mut out: HashMap<&'static str, Hsla> = HashMap::new();
    for (key, value) in &root {
        if key == "palette" {
            continue;
        }
        let color_token: Option<&str> = match value {
            toml::Value::String(s) => Some(s.as_str()),
            toml::Value::Table(t) => t.get("fg").and_then(|v| v.as_str()),
            _ => None,
        };
        let Some(token) = color_token else { continue };
        let Some(rgba) = resolve(token) else { continue };
        let static_key: &'static str = Box::leak(key.clone().into_boxed_str());
        out.insert(static_key, rgba.into());
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
    fn fallback_chain_terminates_at_default_fg() {
        assert_eq!(color_for("totally.unknown"), default_fg());
        assert_eq!(color_for("nope"), default_fg());
    }

    #[test]
    fn onedark_provides_color_for_common_rust_names() {
        for name in &["keyword", "string", "comment", "function", "type"] {
            let c = color_for(name);
            assert_ne!(c, default_fg(), "onedark 必须给 `{name}` 上色");
        }
    }

    #[test]
    fn dot_prefix_fallback_uses_parent_rule() {
        assert_eq!(color_for("function.method"), color_for("function"));
        assert_eq!(color_for("type.builtin"), color_for("type"));
        assert_eq!(color_for("comment.documentation"), color_for("comment"));
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
        assert_eq!(*table.get("keyword").unwrap(), Hsla::from(rgb(0xff0000)));
        assert_eq!(*table.get("string").unwrap(), Hsla::from(rgb(0x00ff00)));
        assert_eq!(*table.get("comment").unwrap(), Hsla::from(rgb(0xabcdef)));
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
        assert_ne!(color_for("variable.parameter"), default_fg());
    }

    #[test]
    fn lsp_method_falls_back_to_function() {
        assert_eq!(color_for("function.method"), color_for("function"));
    }

    #[test]
    fn lsp_enum_member_resolves_via_variable_other_member() {
        assert_ne!(color_for("variable.other.member"), default_fg());
    }

    #[test]
    fn lsp_macro_resolves_via_function_dot_macro() {
        assert_ne!(color_for("function.macro"), default_fg());
    }
}
