//! 设置系统：用户设置文件的解析、合并与变更监听。
//! 此文件是 `zcv-settings` crate 的公共入口。
//!
//! 用户设置 JSON 经 fs watcher 监听，变更防抖重载后写入 SettingsStore global；
//! 默认值与各领域设置由本模块统一提供；具体的运行时类型转换由消费方完成。

use std::borrow::Cow;
use std::path::Path;
use std::sync::LazyLock;

mod file;
mod merge;
mod paths;
mod reload;
mod schema;
mod store;

pub use file::ensure_user_settings_file;
pub use merge::UserSettings;
pub use reload::init;
pub use schema::{SoftWrapMode, TabConfig, TabOverride};
pub use store::{GlobalSettingsErrorReporter, SettingsError, SettingsErrorReporter, SettingsStore};

/// 配置目录（用户主目录下的 `.zcv`）解析入口。
pub fn config_dir() -> &'static Path {
    paths::config_dir()
}

pub(crate) static INITIAL_USER_SETTINGS: LazyLock<Cow<'static, str>> = LazyLock::new(|| {
    zcv_assets::text("settings/initial_user_settings.json").expect("内置初始设置应存在")
});

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::file::ensure_settings_file;
    use crate::schema::parse_user_settings;

    #[test]
    fn missing_user_settings_file_is_created_from_builtin_content() {
        let directory = tempfile::tempdir().expect("应创建临时设置目录");
        let path = directory.path().join("settings.json");

        ensure_settings_file(&path, &INITIAL_USER_SETTINGS).expect("应创建用户设置文件");

        assert_eq!(
            fs::read_to_string(path).expect("应读取新建的用户设置文件"),
            INITIAL_USER_SETTINGS.as_ref()
        );
    }

    #[test]
    fn existing_user_settings_file_is_not_overwritten() {
        let directory = tempfile::tempdir().expect("应创建临时设置目录");
        let path = directory.path().join("settings.json");
        fs::write(&path, r#"{"theme":"one-dark"}"#).expect("应创建用户设置文件");

        ensure_settings_file(&path, &INITIAL_USER_SETTINGS).expect("已有设置文件应可重复初始化");

        assert_eq!(
            fs::read_to_string(path).expect("应读取用户设置文件"),
            r#"{"theme":"one-dark"}"#
        );
    }

    #[test]
    fn missing_fields_use_defaults() {
        let content = parse_user_settings(r#"{"theme":"one-light"}"#).unwrap();
        let settings = UserSettings::merge(content);
        assert_eq!(settings.theme, "one-light");
        assert_eq!(settings.soft_wrap, SoftWrapMode::EditorWidth);
        assert_eq!(settings.preferred_line_length, 80);
        assert!(
            settings
                .file_scan_exclusions
                .iter()
                .any(|glob| glob == "**/.git"),
            "默认排除名单应包含 VCS 目录"
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
                "theme": "one-dark",
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
    fn invalid_field_value_falls_back_to_default() {
        // 非法值字段回退为未配置，由 merge 层用内置默认补齐。
        let settings = UserSettings::merge(parse_user_settings(r#"{"soft_wrap": true}"#).unwrap());
        assert_eq!(settings.soft_wrap, SoftWrapMode::EditorWidth);

        let settings = UserSettings::merge(parse_user_settings(r#"{"theme":"unknown"}"#).unwrap());
        assert_eq!(settings.theme, "unknown");

        let settings = UserSettings::merge(
            parse_user_settings(r#"{"file_scan_exclusions":"not-a-list"}"#).unwrap(),
        );
        assert!(
            settings
                .file_scan_exclusions
                .iter()
                .any(|glob| glob == "**/.git"),
            "非数组名单应回退到内置默认名单"
        );
    }

    #[test]
    fn invalid_field_does_not_affect_other_fields() {
        // 坏字段单独回退默认，好字段照常生效。
        let settings = UserSettings::merge(
            parse_user_settings(
                r#"{"soft_wrap":"bogus","theme":"one-dark","file_scan_exclusions":["**/target"]}"#,
            )
            .unwrap(),
        );
        assert_eq!(settings.soft_wrap, SoftWrapMode::EditorWidth);
        assert_eq!(settings.theme, "one-dark");
        assert_eq!(settings.file_scan_exclusions, vec!["**/target".to_string()]);
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
                    "tab_width": 4,
                    "indent_width": 4,
                    "insert_spaces": true,
                    "languages": {
                        "Rust": { "tab_width": 2 },
                        "Go": { "insert_spaces": false }
                    }
                }"#,
            )
            .unwrap(),
        );

        assert_eq!(settings.tab_for_language(Some("Rust")).tab_width(), 2);
        assert_eq!(settings.tab_for_language(Some("Rust")).indent_width(), 4);
        assert!(settings.tab_for_language(Some("Rust")).insert_spaces);
        assert!(!settings.tab_for_language(Some("Go")).insert_spaces);
        assert_eq!(settings.tab_for_language(Some("Unknown")).tab_width(), 4);
        assert_eq!(settings.tab_for_language(None).tab_width(), 4);
    }

    #[test]
    fn bundled_initial_settings_are_valid() {
        let content = parse_user_settings(&INITIAL_USER_SETTINGS).unwrap();
        assert_eq!(UserSettings::merge(content), UserSettings::default());
    }

    #[test]
    fn invalid_json_reports_location() {
        let error = parse_user_settings(r#"{"theme":}"#).unwrap_err();
        let detailed = format!("{error:#}");
        assert!(detailed.contains("line 1 column"));
    }
}
