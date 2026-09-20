use super::*;

fn style_for_theme(theme_id: &str, name: &str) -> HighlightStyle {
    let theme = theme_by_id(theme_id).expect("内置主题应存在");
    style_for_table(theme.syntax_table.as_ref(), name)
}

/// 语法样式从 App 级 global 读取：切换主题后同一 capture 样式随之变化，不使用进程静态。
#[gpui::test]
fn style_table_reads_app_global_theme(cx: &mut gpui::TestAppContext) {
    let names: Vec<Arc<str>> = vec!["keyword".into()];
    cx.update(|cx| {
        let dark = theme_by_id("dark").expect("内置深色主题应存在");
        set_theme(dark, cx);
        let dark_color = style_table(&names, cx)[0].color;
        let light = theme_by_id("light").expect("内置浅色主题应存在");
        set_theme(light, cx);
        let light_color = style_table(&names, cx)[0].color;
        assert!(dark_color.is_some() && light_color.is_some());
        assert_ne!(dark_color, light_color, "语法样式应随 App 级主题切换");
    });
}

#[test]
fn fallback_chain_terminates_at_default_style() {
    assert_eq!(
        style_for_theme("dark", "totally.unknown"),
        HighlightStyle::default()
    );
    assert_eq!(style_for_theme("dark", "nope"), HighlightStyle::default());
}

#[test]
fn dark_provides_color_for_common_rust_names() {
    for name in &["keyword", "string", "comment", "function", "type"] {
        assert!(
            style_for_theme("dark", name).color.is_some(),
            "dark 必须给 `{name}` 上色"
        );
    }
}

#[test]
fn dot_prefix_fallback_uses_parent_rule() {
    assert_eq!(
        style_for_theme("dark", "function.method").color,
        style_for_theme("dark", "function").color
    );
    assert_eq!(
        style_for_theme("dark", "type.builtin").color,
        style_for_theme("dark", "type").color
    );
    assert_eq!(
        style_for_theme("dark", "comment.documentation").color,
        style_for_theme("dark", "comment").color
    );
}

#[test]
fn lsp_parameter_resolves_to_color_via_variable_parameter() {
    assert!(
        style_for_theme("dark", "variable.parameter")
            .color
            .is_some()
    );
}

#[test]
fn lsp_method_falls_back_to_function() {
    assert_eq!(
        style_for_theme("dark", "function.method").color,
        style_for_theme("dark", "function").color
    );
}

#[test]
fn lsp_enum_member_resolves_via_variable_other_member() {
    assert!(
        style_for_theme("dark", "variable.other.member")
            .color
            .is_some()
    );
}

#[test]
fn lsp_macro_resolves_via_function_dot_macro() {
    assert!(style_for_theme("dark", "function.macro").color.is_some());
}

#[test]
fn project_query_capture_names_resolve_to_theme_colors() {
    for theme_id in ["dark", "light"] {
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
            "selector.class",
            "lifetime",
            "text.jsx",
            "embedded",
        ] {
            assert!(
                style_for_theme(theme_id, name).color.is_some(),
                "{theme_id} 主题必须为项目查询 capture `{name}` 提供颜色"
            );
        }
    }
}

#[test]
fn markdown_capture_rules_keep_theme_modifiers() {
    assert_eq!(
        style_for_theme("dark", "text.strong").font_weight,
        Some(gpui::FontWeight::BOLD)
    );
    assert_eq!(
        style_for_theme("dark", "text.emphasis").font_style,
        Some(gpui::FontStyle::Italic)
    );
    assert!(style_for_theme("dark", "text.title").color.is_some());
}
