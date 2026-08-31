//! 语法高亮：tree-sitter capture name → GPUI HighlightStyle。
//!
//! 本模块只提供查询机制，不定义色值。色值来自主题 TOML，由 `theme_data` 单一解析器解析后经 `set_theme` 注入。
//! 查询走点分前缀回退：`keyword.control.import` 未命中 → `keyword.control` → `keyword` → 默认样式。

use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::{Arc, LazyLock, RwLock};

use gpui::HighlightStyle;

use crate::theme_data::ThemeData;
#[cfg(test)]
use crate::theme_data::theme_by_id;

/// 预展开 capture 名字表为按索引直接取用的样式表。
///
/// 每个名字做一次点分前缀回退；
/// 渲染侧按 capture index 一次数组索引，不再逐 run 做字符串查找与回退。
pub fn style_table(names: &[Arc<str>]) -> Vec<HighlightStyle> {
    names.iter().map(|name| style_for(name)).collect()
}

/// 按 capture name 解析完整样式，走点分前缀回退（一次 BTreeMap range 查询）。
pub(crate) fn style_for(name: &str) -> HighlightStyle {
    // range 覆盖「首段 … 全名」：命中候选都是 name 的前缀，rfind 取最长（最深）的一条。
    let first_segment = name.split('.').next().unwrap_or(name);
    let Ok(theme) = ACTIVE_THEME.read() else {
        return HighlightStyle::default();
    };
    theme
        .range::<str, _>((Bound::Included(first_segment), Bound::Included(name)))
        .rfind(|(prefix, _)| {
            name.strip_prefix(*prefix)
                .is_some_and(|remainder| remainder.is_empty() || remainder.starts_with('.'))
        })
        .map(|(_, style)| *style)
        .unwrap_or_default()
}

/// 注入主题的语法高亮表（主题切换时由 [`ThemeChoice::apply`] 调用）。
pub(crate) fn set_theme(theme: &ThemeData) {
    match ACTIVE_THEME.write() {
        Ok(mut active) => *active = Arc::clone(&theme.syntax_table),
        Err(error) => eprintln!("更新语法主题失败：{error}"),
    }
}

/// 当前主题的高亮表（由 `theme_data` 解析，主题切换时整体替换）。
static ACTIVE_THEME: LazyLock<RwLock<Arc<BTreeMap<&'static str, HighlightStyle>>>> =
    LazyLock::new(|| RwLock::new(Arc::new(BTreeMap::new())));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_chain_terminates_at_default_style() {
        assert_eq!(style_for("totally.unknown"), HighlightStyle::default());
        assert_eq!(style_for("nope"), HighlightStyle::default());
    }

    #[test]
    fn dark_provides_color_for_common_rust_names() {
        set_theme(theme_by_id("dark").expect("内置深色主题应存在"));
        for name in &["keyword", "string", "comment", "function", "type"] {
            assert!(style_for(name).color.is_some(), "dark 必须给 `{name}` 上色");
        }
    }

    #[test]
    fn dot_prefix_fallback_uses_parent_rule() {
        set_theme(theme_by_id("dark").expect("内置深色主题应存在"));
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
        set_theme(theme_by_id("dark").expect("内置深色主题应存在"));
        assert!(style_for("variable.parameter").color.is_some());
    }

    #[test]
    fn lsp_method_falls_back_to_function() {
        set_theme(theme_by_id("dark").expect("内置深色主题应存在"));
        assert_eq!(
            style_for("function.method").color,
            style_for("function").color
        );
    }

    #[test]
    fn lsp_enum_member_resolves_via_variable_other_member() {
        set_theme(theme_by_id("dark").expect("内置深色主题应存在"));
        assert!(style_for("variable.other.member").color.is_some());
    }

    #[test]
    fn lsp_macro_resolves_via_function_dot_macro() {
        set_theme(theme_by_id("dark").expect("内置深色主题应存在"));
        assert!(style_for("function.macro").color.is_some());
    }

    #[test]
    fn project_query_capture_names_resolve_to_theme_colors() {
        for theme_id in ["dark", "light"] {
            set_theme(theme_by_id(theme_id).expect("内置主题应存在"));
            for name in [
                "number",
                "boolean",
                "property.json_key",
                "function.definition",
                "function.special.definition",
                "keyword.declaration",
                "keyword.import",
                "tag.component.jsx",
                "attribute.jsx",
                "lifetime",
                "text.jsx",
                "embedded",
            ] {
                assert!(
                    style_for(name).color.is_some(),
                    "{theme_id} 主题必须为项目查询 capture `{name}` 提供颜色"
                );
            }
        }
    }

    #[test]
    fn markdown_capture_rules_keep_theme_modifiers() {
        set_theme(theme_by_id("dark").expect("内置深色主题应存在"));
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
