//! 主题数据：单一解析器与主题注册表。
//!
//! 对齐 Zed 的 `ThemeRegistry`：每个内置主题统一解析为语义色与语法高亮表，color / syntax 各模块只消费解析结果。
//!
//! 新增内置主题只需提供主题描述符并登记到 `THEMES`，其余模块无需改动。

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use gpui::{
    FontStyle, FontWeight, HighlightStyle, Hsla, Rgba, StrikethroughStyle, UnderlineStyle,
    WindowAppearance, px, rgb, rgba,
};

use crate::color::ThemeColors;

/// 单个已解析主题：元数据 + 语义色 + 语法高亮表。
pub(crate) struct ThemeData {
    pub(crate) id: &'static str,
    /// 主题明暗（`System` 选择时按窗口外观匹配）。
    pub(crate) appearance: WindowAppearance,
    pub(crate) colors: ThemeColors,
    pub(crate) syntax_table: Arc<BTreeMap<&'static str, HighlightStyle>>,
}

/// 主题注册表（编译期内嵌）：新增主题在此登记一行。
static THEMES: OnceLock<Vec<ThemeData>> = OnceLock::new();

pub(crate) fn themes() -> &'static [ThemeData] {
    THEMES.get_or_init(|| {
        // 登记表：文件名 → 嵌入资源路径；主题 id 取自文件名（去掉扩展名），改名或新增主题只动这里与主题文件本身。
        [
            ("dark.toml", "themes/dark.toml"),
            ("light.toml", "themes/light.toml"),
        ]
        .into_iter()
        .map(|(file, path)| {
            let id = file
                .strip_suffix(".toml")
                .expect("主题文件名应以 .toml 结尾");
            let source = zcv_assets::text(path).expect("内嵌主题文件应存在");
            build_theme(id, &source)
        })
        .collect()
    })
}

/// 按 id 查询主题；未知 id 返回 `None`。
pub(crate) fn theme_by_id(id: &str) -> Option<&'static ThemeData> {
    themes().iter().find(|theme| theme.id == id)
}

fn build_theme(id: &'static str, source: &str) -> ThemeData {
    // 内嵌数据错误属于构建期缺陷：直接 panic 暴露，而不是运行时降级掩盖。
    parse_theme(id, source).expect("内嵌主题文件应可解析")
}

/// 单一解析器：一次 TOML 解析产出主题的全部数据。
fn parse_theme(id: &'static str, source: &str) -> Option<ThemeData> {
    let root: toml::Table = toml::from_str(source).ok()?;
    Some(ThemeData {
        id,
        appearance: parse_appearance(&root)?,
        colors: parse_colors(root.get("colors")?.as_table()?)?,
        syntax_table: Arc::new(parse_syntax_table(&root)?),
    })
}

/// 解析主题明暗声明（对齐 Zed 主题文件的 appearance 字段）。
fn parse_appearance(root: &toml::Table) -> Option<WindowAppearance> {
    match root.get("appearance")?.as_str()? {
        "dark" => Some(WindowAppearance::Dark),
        "light" => Some(WindowAppearance::Light),
        _ => None,
    }
}

/// 解析 `[colors]` 段：主题文件直接定义的语义色（对齐 Zed themes 的 style 段）。
/// 全量必填：任一 key 缺失或色值非法即解析失败。
fn parse_colors(colors: &toml::Table) -> Option<ThemeColors> {
    let parse = |key: &str| -> Option<gpui::Rgba> {
        let hex = colors.get(key)?.as_str()?;
        parse_hex(hex)
    };
    Some(ThemeColors {
        background: parse("background")?,
        surface_background: parse("surface.background")?,
        elevated_surface_background: parse("elevated_surface.background")?,
        ghost_element_background: parse("ghost_element.background")?,
        ghost_element_hover: parse("ghost_element.hover")?,
        element_hover: parse("element.hover")?,
        element_selected: parse("element.selected")?,
        border: parse("border")?,
        border_variant: parse("border.variant")?,
        border_focused: parse("border.focused")?,
        text: parse("text")?,
        text_muted: parse("text.muted")?,
        text_disabled: parse("text.disabled")?,
        text_placeholder: parse("text.placeholder")?,
        icon: parse("icon")?,
        icon_muted: parse("icon.muted")?,
        icon_on_accent: parse("icon.on_accent")?,
        icon_accent: parse("icon.accent")?,
        status_success: parse("success")?,
        status_error: parse("error")?,
        status_created: parse("created")?,
        status_modified: parse("modified")?,
        status_deleted: parse("deleted")?,
        status_conflict: parse("conflict")?,
        title_bar_background: parse("title_bar.background")?,
        status_bar_background: parse("status_bar.background")?,
        tab_bar_background: parse("tab_bar.background")?,
        tab_active_background: parse("tab.active_background")?,
        toolbar_background: parse("toolbar.background")?,
        panel_background: parse("panel.background")?,
        editor_background: parse("editor.background")?,
        editor_subheader_background: parse("editor.subheader.background")?,
        editor_active_line_background: parse("editor.active_line.background")?,
        editor_line_number: parse("editor.line_number")?,
        editor_active_line_number: parse("editor.active_line_number")?,
        editor_selection_background: parse("editor.selection.background")?,
        search_match_background: parse("search.match_background")?,
        search_active_match_background: parse("search.active_match_background")?,
        editor_cursor: parse("editor.cursor")?,
        editor_diff_added_background: parse("editor.diff_hunk.added_background")?,
        editor_diff_deleted_background: parse("editor.diff_hunk.deleted_background")?,
        scrollbar_track_background: parse("scrollbar.track.background")?,
        scrollbar_thumb_background: parse("scrollbar.thumb.background")?,
        scrollbar_thumb_hover_background: parse("scrollbar.thumb.hover_background")?,
        scrollbar_thumb_active_background: parse("scrollbar.thumb.active_background")?,
        // 主色 8 个。
        terminal_ansi_black: parse("terminal.ansi.black")?,
        terminal_ansi_red: parse("terminal.ansi.red")?,
        terminal_ansi_green: parse("terminal.ansi.green")?,
        terminal_ansi_yellow: parse("terminal.ansi.yellow")?,
        terminal_ansi_blue: parse("terminal.ansi.blue")?,
        terminal_ansi_magenta: parse("terminal.ansi.magenta")?,
        terminal_ansi_cyan: parse("terminal.ansi.cyan")?,
        terminal_ansi_white: parse("terminal.ansi.white")?,
        // 亮色变体 8 个。
        terminal_ansi_bright_black: parse("terminal.ansi.bright_black")?,
        terminal_ansi_bright_red: parse("terminal.ansi.bright_red")?,
        terminal_ansi_bright_green: parse("terminal.ansi.bright_green")?,
        terminal_ansi_bright_yellow: parse("terminal.ansi.bright_yellow")?,
        terminal_ansi_bright_blue: parse("terminal.ansi.bright_blue")?,
        terminal_ansi_bright_magenta: parse("terminal.ansi.bright_magenta")?,
        terminal_ansi_bright_cyan: parse("terminal.ansi.bright_cyan")?,
        terminal_ansi_bright_white: parse("terminal.ansi.bright_white")?,
        // 暗化变体 8 个。
        terminal_ansi_dim_black: parse("terminal.ansi.dim_black")?,
        terminal_ansi_dim_red: parse("terminal.ansi.dim_red")?,
        terminal_ansi_dim_green: parse("terminal.ansi.dim_green")?,
        terminal_ansi_dim_yellow: parse("terminal.ansi.dim_yellow")?,
        terminal_ansi_dim_blue: parse("terminal.ansi.dim_blue")?,
        terminal_ansi_dim_magenta: parse("terminal.ansi.dim_magenta")?,
        terminal_ansi_dim_cyan: parse("terminal.ansi.dim_cyan")?,
        terminal_ansi_dim_white: parse("terminal.ansi.dim_white")?,
    })
}

/// 解析 `[palette]` 颜色名引用与各高亮规则段（Helix 风格主题）。
///
/// 支持 `"name" = "color"` 和 `"name" = { fg = "color", modifiers = [...] }` 两种格式；
/// 不含 `fg` 的条目跳过，颜色名引用解析自 `[palette]` 段。
fn parse_syntax_table(root: &toml::Table) -> Option<BTreeMap<&'static str, HighlightStyle>> {
    let palette: BTreeMap<String, Rgba> = root
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

    let mut out: BTreeMap<&'static str, HighlightStyle> = BTreeMap::new();
    for (key, value) in root {
        // 主题元数据段不参与语法规则解析（防未来 [palette] 出现同名颜色名时误入语法表）。
        if matches!(key.as_str(), "palette" | "colors" | "appearance") {
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
    fn theme_registry_has_expected_entries() {
        let themes = themes();
        assert_eq!(themes.len(), 2);
        assert_eq!(themes[0].id, "dark");
        assert_eq!(themes[1].id, "light");
        // 主题明暗声明来自文件而非硬编码。
        assert_eq!(themes[0].appearance, WindowAppearance::Dark);
        assert_eq!(themes[1].appearance, WindowAppearance::Light);
        assert!(theme_by_id("unknown").is_none());
    }

    /// 语义色由主题文件直接定义：抽样断言关键表面色与 Zed 官方 One Dark 一致。
    #[test]
    fn dark_theme_colors_match_migrated_values() {
        let theme = theme_by_id("dark").expect("内置深色主题应存在");
        let colors = theme.colors;
        assert_eq!(colors.background, gpui::rgba(0x3b414dff));
        assert_eq!(colors.status_bar_background, gpui::rgba(0x3b414dff));
        assert_eq!(colors.panel_background, gpui::rgba(0x2f343eff));
        assert_eq!(colors.editor_background, gpui::rgba(0x282c33ff));
        assert_eq!(colors.editor_subheader_background, gpui::rgba(0x2f343eff));
        assert_eq!(colors.text, gpui::rgba(0xdce0e5ff));
        assert_eq!(colors.editor_selection_background, gpui::rgba(0x74ade81a));
        assert_eq!(
            colors.scrollbar_thumb_active_background,
            gpui::rgba(0x363c46ff)
        );
        // 终端 ANSI 色与 Zed 官方一致。
        assert_eq!(colors.terminal_ansi_red, gpui::rgba(0xe06c75ff));
        assert_eq!(colors.terminal_ansi_yellow, gpui::rgba(0xe5c07bff));
        assert_eq!(colors.terminal_ansi_dim_blue, gpui::rgba(0x457cadff));
    }

    #[test]
    fn light_theme_colors_match_migrated_values() {
        let theme = theme_by_id("light").expect("内置浅色主题应存在");
        let colors = theme.colors;
        assert_eq!(colors.background, gpui::rgba(0xdcdcddff));
        assert_eq!(colors.status_bar_background, gpui::rgba(0xdcdcddff));
        assert_eq!(colors.editor_background, gpui::rgba(0xfafafaff));
        assert_eq!(colors.editor_subheader_background, gpui::rgba(0xebebecff));
        assert_eq!(colors.text, gpui::rgba(0x242529ff));
        assert_eq!(colors.editor_selection_background, gpui::rgba(0x5c78e225));
        assert_eq!(colors.ghost_element_hover, gpui::rgba(0xc9c9caff));
        // 终端 ANSI 色与 Zed 官方一致。
        assert_eq!(colors.terminal_ansi_yellow, gpui::rgba(0xd2b67cff));
        assert_eq!(colors.terminal_ansi_blue, gpui::rgba(0x2f5af3ff));
    }

    /// 最小合法主题：元数据 + 语法规则 + 语义色，供解析失败族测试破坏单点。
    fn minimal_theme() -> String {
        r##"
            appearance = "dark"
            "keyword" = "red"
            [palette]
            red = "#ff0000"
            [colors]
            background = "#000000ff"
            "surface.background" = "#111111ff"
            "elevated_surface.background" = "#222222ff"
            "element.hover" = "#333333ff"
            "element.selected" = "#333333ff"
            "border.variant" = "#444444ff"
            "border.focused" = "#555555ff"
            text = "#666666ff"
            "text.muted" = "#777777ff"
            "text.disabled" = "#888888ff"
            "text.placeholder" = "#999999ff"
            icon = "#777777ff"
            "icon.muted" = "#888888ff"
            "icon.on_accent" = "#000000ff"
            "icon.accent" = "#555555ff"
            success = "#00ff00ff"
            error = "#ff0000ff"
            created = "#00ff00ff"
            modified = "#ffff00ff"
            deleted = "#ff0000ff"
            conflict = "#ff0000ff"
            "title_bar.background" = "#222222ff"
            "status_bar.background" = "#111111ff"
            "tab_bar.background" = "#222222ff"
            "tab.active_background" = "#111111ff"
            "toolbar.background" = "#111111ff"
            "panel.background" = "#222222ff"
            "editor.background" = "#333333ff"
            "editor.subheader.background" = "#292929ff"
            "editor.active_line.background" = "#33333380"
            "editor.line_number" = "#888888ff"
            "editor.active_line_number" = "#666666ff"
            "editor.selection.background" = "#5555553d"
            "search.match_background" = "#5555558c"
            "search.active_match_background" = "#ffff0066"
            "editor.cursor" = "#555555ff"
            "editor.diff_hunk.added_background" = "#00ff004d"
            "editor.diff_hunk.deleted_background" = "#ff00004d"
            "scrollbar.track.background" = "#00000000"
            "scrollbar.thumb.background" = "#88888873"
            "scrollbar.thumb.hover_background" = "#8888888c"
            "scrollbar.thumb.active_background" = "#888888a6"
            "terminal.ansi.black" = "#000000ff"
            "terminal.ansi.red" = "#ff0000ff"
            "terminal.ansi.green" = "#00ff00ff"
            "terminal.ansi.yellow" = "#ffff00ff"
            "terminal.ansi.blue" = "#0000ffff"
            "terminal.ansi.magenta" = "#ff00ffff"
            "terminal.ansi.cyan" = "#00ffffff"
            "terminal.ansi.white" = "#ffffffff"
            "terminal.ansi.bright_black" = "#000000ff"
            "terminal.ansi.bright_red" = "#ff0000ff"
            "terminal.ansi.bright_green" = "#00ff00ff"
            "terminal.ansi.bright_yellow" = "#ffff00ff"
            "terminal.ansi.bright_blue" = "#0000ffff"
            "terminal.ansi.bright_magenta" = "#ff00ffff"
            "terminal.ansi.bright_cyan" = "#00ffffff"
            "terminal.ansi.bright_white" = "#ffffffff"
            "terminal.ansi.dim_black" = "#000000ff"
            "terminal.ansi.dim_red" = "#ff0000ff"
            "terminal.ansi.dim_green" = "#00ff00ff"
            "terminal.ansi.dim_yellow" = "#ffff00ff"
            "terminal.ansi.dim_blue" = "#0000ffff"
            "terminal.ansi.dim_magenta" = "#ff00ffff"
            "terminal.ansi.dim_cyan" = "#00ffffff"
            "terminal.ansi.dim_white" = "#ffffffff"
        "##
        .to_string()
    }

    #[test]
    fn parse_theme_rejects_missing_color_key() {
        let src = minimal_theme().replace("            \"editor.cursor\" = \"#555555ff\"\n", "");
        assert!(parse_theme("test", &src).is_none());
    }

    #[test]
    fn parse_theme_rejects_invalid_hex() {
        let src = minimal_theme().replace("#000000ff", "#12zz34ff");
        assert!(parse_theme("test", &src).is_none());
    }

    #[test]
    fn parse_theme_rejects_unknown_appearance() {
        let src = minimal_theme().replace("appearance = \"dark\"", "appearance = \"blue\"");
        assert!(parse_theme("test", &src).is_none());
    }

    #[test]
    fn parse_theme_rejects_unquoted_dotted_keys() {
        // 未加引号的点分 key 会解析为嵌套表，语义色取值落空 → 解析失败。
        let src = minimal_theme().replace(
            "\"surface.background\" = \"#111111ff\"",
            "surface.background = \"#111111ff\"",
        );
        assert!(parse_theme("test", &src).is_none());
    }

    #[test]
    fn syntax_table_ignores_theme_meta_sections() {
        let table = parse_syntax_table(&toml::from_str(&minimal_theme()).expect("应可解析"))
            .expect("应可解析");
        // 元数据段（appearance / colors）不进入语法规则表。
        assert_eq!(table.len(), 1);
        assert!(table.contains_key("keyword"));
    }

    #[test]
    fn syntax_table_resolves_palette_references() {
        let src = r##"
            "keyword" = { fg = "red" }
            "string" = "green"
            "comment" = "#abcdef"
            [palette]
            red = "#ff0000"
            green = "#00ff00"
        "##;
        let table = parse_syntax_table(&toml::from_str(src).expect("应能解析")).expect("应能解析");
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
    fn syntax_table_skips_entries_without_fg() {
        let src = r##"
            "ui.background" = { bg = "black" }
            "diagnostic.unnecessary" = { modifiers = ["dim"] }
            "keyword" = { fg = "red" }
            [palette]
            red = "#ff0000"
            black = "#000000"
        "##;
        let table = parse_syntax_table(&toml::from_str(src).expect("应能解析")).expect("应能解析");
        assert!(!table.contains_key("ui.background"));
        assert!(!table.contains_key("diagnostic.unnecessary"));
        assert!(table.contains_key("keyword"));
    }

    #[test]
    fn syntax_table_skips_unresolved_color_tokens() {
        let src = r##"
            "keyword" = "missing-color"
            "string" = "red"
            [palette]
            red = "#ff0000"
        "##;
        let table = parse_syntax_table(&toml::from_str(src).expect("应能解析")).expect("应能解析");
        assert!(!table.contains_key("keyword"));
        assert!(table.contains_key("string"));
    }
}
