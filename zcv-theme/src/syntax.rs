//! 语法高亮：tree-sitter capture name → GPUI HighlightStyle。
//!
//! 本模块只提供查询机制，不定义色值。色值来自主题 TOML，由 [`crate::theme_data`] 单一解析器解析后经 `set_theme` 注入。
//! 查询走点分前缀回退：`keyword.control.import` 未命中 → `keyword.control` → `keyword` → [`default_fg`]。

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use gpui::{HighlightStyle, Hsla};

use super::color;
use crate::theme_data::ThemeData;

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

/// 注入主题的语法高亮表（主题切换时由 [`ThemeChoice::apply`] 调用）。
pub(crate) fn set_theme(theme: &ThemeData) {
    let lock = ACTIVE_THEME.get_or_init(|| RwLock::new(Arc::new(HashMap::new())));
    match lock.write() {
        Ok(mut active) => *active = Arc::clone(&theme.syntax_table),
        Err(error) => eprintln!("更新语法主题失败：{error}"),
    }
}

fn lookup_in_theme(name: &str) -> Option<HighlightStyle> {
    let lock = ACTIVE_THEME.get_or_init(|| RwLock::new(Arc::new(HashMap::new())));
    lock.read().ok().and_then(|theme| theme.get(name).copied())
}

/// 当前主题的高亮表（由 [`crate::theme_data`] 解析，主题切换时整体替换）。
static ACTIVE_THEME: OnceLock<RwLock<Arc<HashMap<&'static str, HighlightStyle>>>> = OnceLock::new();

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
        set_theme(crate::theme_data::theme_by_id("one-dark").expect("内置 onedark 主题应存在"));
        for name in &["keyword", "string", "comment", "function", "type"] {
            assert!(
                style_for(name).color.is_some(),
                "onedark 必须给 `{name}` 上色"
            );
        }
    }

    #[test]
    fn dot_prefix_fallback_uses_parent_rule() {
        set_theme(crate::theme_data::theme_by_id("one-dark").expect("内置 onedark 主题应存在"));
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
    fn lsp_parameter_resolves_to_color_via_variable_parameter() {
        set_theme(crate::theme_data::theme_by_id("one-dark").expect("内置 onedark 主题应存在"));
        assert!(style_for("variable.parameter").color.is_some());
    }

    #[test]
    fn lsp_method_falls_back_to_function() {
        set_theme(crate::theme_data::theme_by_id("one-dark").expect("内置 onedark 主题应存在"));
        assert_eq!(
            style_for("function.method").color,
            style_for("function").color
        );
    }

    #[test]
    fn lsp_enum_member_resolves_via_variable_other_member() {
        set_theme(crate::theme_data::theme_by_id("one-dark").expect("内置 onedark 主题应存在"));
        assert!(style_for("variable.other.member").color.is_some());
    }

    #[test]
    fn lsp_macro_resolves_via_function_dot_macro() {
        set_theme(crate::theme_data::theme_by_id("one-dark").expect("内置 onedark 主题应存在"));
        assert!(style_for("function.macro").color.is_some());
    }

    #[test]
    fn markdown_capture_rules_keep_theme_modifiers() {
        set_theme(crate::theme_data::theme_by_id("one-dark").expect("内置 onedark 主题应存在"));
        assert_eq!(
            style_for("text.strong").font_weight,
            Some(gpui::FontWeight::BOLD)
        );
        assert_eq!(
            style_for("text.emphasis").font_style,
            Some(gpui::FontStyle::Italic)
        );
        assert!(style_for("text.title").color.is_some());
    }
}
