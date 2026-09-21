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

/// 语义色由主题文件直接定义：抽样断言关键表面色。
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
    assert_eq!(colors.editor_selection_background, gpui::rgba(0x74ade83d));
    assert_eq!(colors.editor_invisible, gpui::rgba(0x4e5a5fff));
    assert_eq!(
        colors.editor_document_highlight_bracket_background,
        gpui::rgba(0x74ade81a)
    );
    assert_ne!(
        colors.editor_document_highlight_bracket_background,
        colors.editor_selection_background
    );
    assert_eq!(
        colors.scrollbar_thumb_active_background,
        gpui::rgba(0x363c46ff)
    );
    assert_eq!(colors.version_control_word_added, gpui::rgba(0x2EA04859));
    assert_eq!(colors.version_control_word_deleted, gpui::rgba(0xe06c76cc));
    assert_ne!(
        colors.version_control_word_added,
        colors.editor_diff_added_background
    );
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
    assert_eq!(colors.editor_selection_background, gpui::rgba(0x5c78e23d));
    assert_eq!(colors.editor_invisible, gpui::rgba(0xb4b4bbff));
    assert_eq!(
        colors.editor_document_highlight_bracket_background,
        gpui::rgba(0x5c78e225)
    );
    assert_ne!(
        colors.editor_document_highlight_bracket_background,
        colors.editor_selection_background
    );
    assert_eq!(colors.ghost_element_hover, gpui::rgba(0xc9c9caff));
    assert_eq!(colors.version_control_word_added, gpui::rgba(0x2EA04859));
    assert_eq!(colors.version_control_word_deleted, gpui::rgba(0xe06c76cc));
    assert_ne!(
        colors.version_control_word_added,
        colors.editor_diff_added_background
    );
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
        "ghost_element.hover" = "#333333ff"
        "element.hover" = "#333333ff"
        "element.selected" = "#333333ff"
        border = "#444444ff"
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
        "version_control.added" = "#00ff00ff"
        "version_control.modified" = "#ffff00ff"
        "version_control.deleted" = "#ff0000ff"
        "version_control.word_added" = "#00ff00ff"
        "version_control.word_deleted" = "#ff0000ff"
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
        "editor.invisible" = "#888888ff"
        "editor.document_highlight.bracket_background" = "#00ff004d"
        "search.match_background" = "#5555558c"
        "search.active_match_background" = "#ffff0066"
        "editor.cursor" = "#555555ff"
        "editor.diff_hunk.added_background" = "#00ff004d"
        "editor.diff_hunk.deleted_background" = "#ff00004d"
        "editor.diff_hunk.added_hollow_border" = "#00ff0080"
        "editor.diff_hunk.deleted_hollow_border" = "#ff000080"
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

/// 基线可解析：最小主题必须与 `parse_colors` 必填键集保持同步，否则下方“拒绝非法主题”一族测试会空转通过。
#[test]
fn minimal_theme_parses() {
    assert!(parse_theme("test", &minimal_theme()).is_some());
}

#[test]
fn parse_theme_rejects_missing_color_key() {
    // 夹具行带 8 空格缩进；这里按 key 值定位，不绑定具体缩进。
    let src = minimal_theme().replace("\"editor.cursor\" = \"#555555ff\"\n", "");
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
    let table =
        parse_syntax_table(&toml::from_str(&minimal_theme()).expect("应可解析")).expect("应可解析");
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
