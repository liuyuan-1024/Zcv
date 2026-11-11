use std::fs;
use zcv_theme::ThemeChoice;

use super::*;
use crate::{
    file::ensure_settings_file,
    schema::{TerminalShellSetting, parse_builtin_settings, parse_user_settings},
};

#[test]
fn missing_user_settings_file_contains_only_overrides() {
    let directory = tempfile::tempdir().expect("应创建临时设置目录");
    let path = directory.path().join("settings.json");

    let template = "// 配置项和当前默认值见同目录的 settings.reference.jsonc。\n{}\n";
    ensure_settings_file(&path, template).expect("应创建用户设置文件");

    assert_eq!(
        fs::read_to_string(path).expect("应读取新建的用户设置文件"),
        template
    );
}

#[test]
fn existing_user_settings_file_is_not_overwritten() {
    let directory = tempfile::tempdir().expect("应创建临时设置目录");
    let path = directory.path().join("settings.json");
    fs::write(&path, r#"{"theme":"dark"}"#).expect("应创建用户设置文件");

    ensure_settings_file(&path, "{}\n").expect("已有设置文件应可重复初始化");

    assert_eq!(
        fs::read_to_string(path).expect("应读取用户设置文件"),
        r#"{"theme":"dark"}"#
    );
}

#[test]
fn missing_fields_use_defaults() {
    let content = parse_user_settings(r#"{"theme":"light"}"#).unwrap();
    let settings = UserSettings::merge(content);
    let mut expected = UserSettings::default();
    assert_ne!(expected.theme, ThemeChoice::Named("light"));
    expected.theme = ThemeChoice::Named("light");
    assert_eq!(settings, expected);
}

#[test]
fn newline_settings_have_explicit_defaults_and_overrides() {
    let defaults = UserSettings::default();
    assert_eq!(defaults.auto_indent, AutoIndentMode::SyntaxAware);
    assert!(defaults.extend_comment_on_newline);
    assert!(defaults.extend_list_on_newline);

    let settings = UserSettings::merge(
        parse_user_settings(
            r#"{
                "auto_indent": "none",
                "extend_comment_on_newline": false,
                "extend_list_on_newline": false
            }"#,
        )
        .unwrap(),
    );
    assert_eq!(settings.auto_indent, AutoIndentMode::None);
    assert!(!settings.extend_comment_on_newline);
    assert!(!settings.extend_list_on_newline);
}

#[test]
fn save_whitespace_settings_have_independent_global_and_language_overrides() {
    let defaults = UserSettings::default();
    assert!(defaults.remove_trailing_whitespace_on_save);
    assert!(defaults.ensure_final_newline_on_save);

    let settings = UserSettings::merge(
        parse_user_settings(
            r#"{
                "remove_trailing_whitespace_on_save": false,
                "languages": {
                    "Rust": {
                        "remove_trailing_whitespace_on_save": true,
                        "ensure_final_newline_on_save": false
                    }
                }
            }"#,
        )
        .unwrap(),
    );
    assert!(!settings.remove_trailing_whitespace_on_save_for_language(None));
    assert!(settings.ensure_final_newline_on_save_for_language(None));
    assert!(settings.remove_trailing_whitespace_on_save_for_language(Some("Rust")));
    assert!(!settings.ensure_final_newline_on_save_for_language(Some("Rust")));
    assert!(!settings.remove_trailing_whitespace_on_save_for_language(Some("Go")));
}

#[test]
fn cursor_shape_setting_supports_all_zed_shapes() {
    for (name, shape) in [
        ("bar", CursorShape::Bar),
        ("block", CursorShape::Block),
        ("underline", CursorShape::Underline),
        ("hollow", CursorShape::Hollow),
    ] {
        let content = parse_user_settings(&format!(r#"{{"cursor_shape":"{name}"}}"#)).unwrap();
        assert_eq!(UserSettings::merge(content).cursor_shape, shape);
    }
}

#[test]
fn highlight_contrast_setting_has_zed_default_and_can_be_disabled() {
    assert_eq!(
        UserSettings::default().minimum_contrast_for_highlights,
        45.0
    );
    let disabled = UserSettings::merge(
        parse_user_settings(r#"{"minimum_contrast_for_highlights":0}"#).unwrap(),
    );
    assert_eq!(disabled.minimum_contrast_for_highlights, 0.0);
    assert!(parse_user_settings(r#"{"minimum_contrast_for_highlights":107}"#).is_err());
    assert!(parse_user_settings(r#"{"minimum_contrast_for_highlights":"45"}"#).is_err());
}

#[test]
fn cursor_blink_and_animation_settings_are_independent() {
    let defaults = UserSettings::default();
    let blink_disabled =
        UserSettings::merge(parse_user_settings(r#"{"cursor_blink":false}"#).unwrap());
    assert!(!blink_disabled.cursor_blink);
    assert_eq!(
        blink_disabled.cursor_animation_enabled,
        defaults.cursor_animation_enabled
    );

    let animation_disabled = UserSettings::merge(
        parse_user_settings(r#"{"cursor_animation":{"enabled":false}}"#).unwrap(),
    );
    assert_eq!(animation_disabled.cursor_blink, defaults.cursor_blink);
    assert!(!animation_disabled.cursor_animation_enabled);

    let nested_field_missing =
        UserSettings::merge(parse_user_settings(r#"{"cursor_animation":{}}"#).unwrap());
    assert_eq!(
        nested_field_missing.cursor_animation_enabled,
        defaults.cursor_animation_enabled
    );
}

#[test]
fn content_and_ui_typography_are_independently_configurable() {
    let content = parse_user_settings(
        r#"{
            "content_font_size": 18,
            "content_line_height": 1.4,
            "ui_font_size": 15
        }"#,
    )
    .unwrap();
    let settings = UserSettings::merge(content);

    assert_eq!(settings.content_font_size, 18.);
    assert_eq!(settings.content_line_height, 1.4);
    assert_eq!(settings.ui_font_size, 15.);
}

#[test]
fn file_scan_exclusions_override_the_default_list() {
    let content =
        parse_user_settings(r#"{"file_scan_exclusions":["**/target","**/.cache"]}"#).unwrap();
    let settings = UserSettings::merge(content);
    assert_eq!(
        settings.file_scan_exclusions,
        vec!["**/target".to_string(), "**/.cache".to_string()]
    );
}

#[test]
fn explicit_empty_exclusions_do_not_fall_back_to_defaults() {
    // 显式写空名单表示用户想清空排除，不应回退到内置默认名单。
    let content = parse_user_settings(r#"{"file_scan_exclusions":[]}"#).unwrap();
    let settings = UserSettings::merge(content);
    assert!(
        settings.file_scan_exclusions.is_empty(),
        "显式空名单应保持为空"
    );
}

#[test]
fn comments_and_trailing_commas_are_supported() {
    let content = parse_user_settings(
        r#"{
            // settings.json 使用 JSONC 语义。
            "theme": "dark",
            "soft_wrap": "editor-width",
        }"#,
    )
    .unwrap();
    assert_eq!(
        UserSettings::merge(content).soft_wrap,
        SoftWrapMode::EditorWidth
    );
}

#[test]
fn invalid_user_field_fails_instead_of_using_default() {
    for content in [
        r#"{"soft_wrap":true}"#,
        r#"{"theme":"unknown"}"#,
        r#"{"file_scan_exclusions":"not-a-list"}"#,
        r#"{"file_scan_exclusions":["["]}"#,
        r#"{"languages":{"Rust":{"insert_spaces":"yes"}}}"#,
        r#"{"cursor_animation":{"enabled":"yes"}}"#,
        r#"{"terminal_cursor_shape":"unknown"}"#,
        r#"{"tab_size":0}"#,
        r#"{"languages":{"Rust":{"tab_size":0}}}"#,
        r#"{"indent_guides":{"line_width":0}}"#,
        r#"{"indent_guides":{"line_width":11}}"#,
        r#"{"languages":{"Rust":{"indent_guides":{"line_width":11}}}}"#,
        r#"{"preferred_line_length":0}"#,
        r#"{"content_font_size":0}"#,
        r#"{"terminal_line_height":-1}"#,
        r#"{"terminal_shell":""}"#,
        r#"{"terminal_shell":true}"#,
        r#"{"soft_warp":"bounded"}"#,
        r#"{"languages":{"Rust":{"tab_szie":4}}}"#,
        r#"{"indent_guides":{"line_wdith":2}}"#,
        r#"{"cursor_animation":{"enabeld":true}}"#,
        r#"{"theme":null}"#,
        r#"{"tab_size":null}"#,
        r#"{"indent_guides":null}"#,
        r#"{"indent_guides":{"line_width":null}}"#,
        r#"{"languages":{"Rust":{"tab_size":null}}}"#,
        r#"{"cursor_animation":{"enabled":null}}"#,
    ] {
        assert!(parse_user_settings(content).is_err(), "应拒绝 {content}");
    }
}

#[test]
fn invalid_user_settings_keep_the_last_valid_snapshot() {
    let mut store = SettingsStore::new(UserSettings::default());
    assert!(
        store
            .set_user_settings(r#"{"soft_wrap":"bounded"}"#)
            .unwrap()
    );
    let before = store.settings.clone();
    assert!(
        store
            .set_user_settings(r#"{"soft_wrap":true,"theme":"light"}"#)
            .is_err()
    );
    assert_eq!(store.settings, before);
}

#[test]
fn soft_wrap_modes_and_preferred_line_length_parse() {
    let content = parse_user_settings(
        r#"{
            "soft_wrap": "bounded",
            "preferred_line_length": 100,
        }"#,
    )
    .unwrap();
    let settings = UserSettings::merge(content);
    assert_eq!(settings.soft_wrap, SoftWrapMode::Bounded);
    assert_eq!(settings.preferred_line_length, 100);

    let content = parse_user_settings(r#"{"soft_wrap": "editor-width"}"#).unwrap();
    let settings = UserSettings::merge(content);
    assert_eq!(settings.soft_wrap, SoftWrapMode::EditorWidth);
    assert_eq!(settings.preferred_line_length, 80);

    let content = parse_user_settings(r#"{"soft_wrap": "none"}"#).unwrap();
    assert_eq!(UserSettings::merge(content).soft_wrap, SoftWrapMode::None);
}

#[test]
fn per_language_overrides_replace_global_tab_fields() {
    let settings = UserSettings::merge(
        parse_user_settings(
            r#"{
                "tab_size": 4,
                "insert_spaces": true,
                "languages": {
                    "Rust": { "tab_size": 2 },
                    "Go": { "insert_spaces": false }
                }
            }"#,
        )
        .unwrap(),
    );

    assert_eq!(settings.tab_for_language(Some("Rust")).tab_size(), 2);
    assert!(settings.tab_for_language(Some("Rust")).insert_spaces);
    assert!(!settings.tab_for_language(Some("Go")).insert_spaces);
    assert_eq!(settings.tab_for_language(Some("Unknown")).tab_size(), 4);
    assert_eq!(settings.tab_for_language(None).tab_size(), 4);
}

#[test]
fn builtin_language_indent_defaults_apply_and_user_overrides_win() {
    // 内置配置文件提供各语言默认缩进。
    let defaults = UserSettings::default();
    assert_eq!(defaults.tab_for_language(Some("JavaScript")).tab_size(), 2);
    assert!(!defaults.tab_for_language(Some("Go")).insert_spaces);
    assert_eq!(defaults.tab_for_language(None).tab_size(), 4);

    // 用户按语言配置覆盖内置默认；未覆盖的字段仍沿用内置值。
    let settings = UserSettings::merge(
        parse_user_settings(r#"{"languages": {"JavaScript": {"tab_size": 8}}}"#).unwrap(),
    );
    assert_eq!(settings.tab_for_language(Some("JavaScript")).tab_size(), 8);
    assert!(!settings.tab_for_language(Some("Go")).insert_spaces);
}

#[test]
fn indent_guide_settings_merge_per_language() {
    let settings = UserSettings::merge(
        parse_user_settings(
            r#"{
                "indent_guides": { "line_width": 2 },
                "languages": {
                    "Rust": { "tab_size": 2, "indent_guides": { "line_width": 3 } }
                }
            }"#,
        )
        .unwrap(),
    );
    let rust = settings.indent_guides_for_language(Some("Rust"));
    assert_eq!(settings.tab_for_language(Some("Rust")).tab_size(), 2);
    assert_eq!(rust.line_width, 3);
    assert_eq!(
        settings.indent_guides_for_language(Some("Go")).line_width,
        2
    );
}

#[test]
fn bundled_initial_settings_are_valid() {
    let content = parse_builtin_settings(&INITIAL_USER_SETTINGS).unwrap();
    let defaults = UserSettings::default();
    assert_eq!(content.tab_size.get(), 4);
    assert!(defaults.cursor_animation_enabled);
    assert_eq!(defaults.tab, TabConfig::default());
    assert_eq!(defaults.indent_guides, IndentGuideSettings::default());
}

#[test]
fn builtin_settings_require_every_field_including_nested_and_nullable_fields() {
    let original: serde_json_lenient::Value =
        serde_json_lenient::from_str(&INITIAL_USER_SETTINGS).unwrap();
    for path in original.as_object().unwrap().keys() {
        let mut value = original.clone();
        value.as_object_mut().unwrap().remove(path);
        let content = serde_json_lenient::to_string(&value).unwrap();
        assert!(
            parse_builtin_settings(&content).is_err(),
            "缺少 {path} 应报错"
        );
    }
    for parent in ["indent_guides", "cursor_animation"] {
        for field in original[parent].as_object().unwrap().keys() {
            let mut value = original.clone();
            value[parent].as_object_mut().unwrap().remove(field);
            let content = serde_json_lenient::to_string(&value).unwrap();
            assert!(
                parse_builtin_settings(&content).is_err(),
                "缺少 {parent}.{field} 应报错"
            );
        }
    }
}

#[test]
fn invalid_builtin_values_fail_instead_of_falling_back() {
    let mut value: serde_json_lenient::Value =
        serde_json_lenient::from_str(&INITIAL_USER_SETTINGS).unwrap();
    for (field, invalid) in [
        ("tab_size", serde_json_lenient::json!(0)),
        ("content_font_size", serde_json_lenient::json!(0)),
        ("terminal_line_height", serde_json_lenient::json!(-1)),
        ("preferred_line_length", serde_json_lenient::json!(0)),
        ("terminal_shell", serde_json_lenient::json!("")),
        ("file_scan_exclusions", serde_json_lenient::json!(["["])),
    ] {
        value[field] = invalid;
        assert!(
            parse_builtin_settings(&serde_json_lenient::to_string(&value).unwrap()).is_err(),
            "应拒绝 {field}"
        );
        value = serde_json_lenient::from_str(&INITIAL_USER_SETTINGS).unwrap();
    }
    value["indent_guides"]["line_width"] = 11.into();
    assert!(parse_builtin_settings(&serde_json_lenient::to_string(&value).unwrap()).is_err());
}

#[test]
fn user_shell_absence_and_explicit_null_are_distinct() {
    assert_eq!(parse_user_settings("{}").unwrap().terminal_shell, None);
    assert_eq!(
        parse_user_settings(r#"{"terminal_shell":null}"#)
            .unwrap()
            .terminal_shell,
        Some(TerminalShellSetting::System)
    );
    assert_eq!(
        parse_user_settings(r#"{"terminal_shell":"/bin/zsh"}"#)
            .unwrap()
            .terminal_shell,
        Some(TerminalShellSetting::Program("/bin/zsh".to_owned()))
    );
}

#[test]
fn invalid_json_reports_location() {
    let error = parse_user_settings(r#"{"theme":}"#).unwrap_err();
    let detailed = format!("{error:#}");
    assert!(detailed.contains("line 1 column"));
}

#[test]
fn explicit_null_reports_location() {
    let error = parse_user_settings("{\n  \"theme\": null\n}").unwrap_err();
    let detailed = format!("{error:#}");
    assert!(detailed.contains("配置项不能为 null"), "{detailed}");
    assert!(detailed.contains("line 3 column 1"), "{detailed}");
}
