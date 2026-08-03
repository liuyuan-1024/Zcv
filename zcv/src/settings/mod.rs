use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result};
use gpui::{App, Global, Task};
use serde::Deserialize;
use zcv_editor::SoftWrap;
use zcv_theme::Theme;

use crate::fs_watcher::{FsWatcher, Watcher};

const INITIAL_USER_SETTINGS: &str =
    include_str!("../../assets/settings/initial_user_settings.json");
const SETTINGS_RELOAD_DEBOUNCE: Duration = Duration::from_millis(75);
const SETTINGS_RELOAD_RETRY_DELAY: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum ThemeContent {
    System,
    OneDark,
    OneLight,
}

impl From<ThemeContent> for Theme {
    fn from(value: ThemeContent) -> Self {
        match value {
            ThemeContent::System => Theme::System,
            ThemeContent::OneDark => Theme::OneDark,
            ThemeContent::OneLight => Theme::OneLight,
        }
    }
}

/// 软换行模式的设置值，语义与 Zed 的 `soft_wrap` 一致。
///
/// - `none`：不换行，超长行靠水平滚动查看；
/// - `editor-width`：行宽超过编辑器文本区宽度时换行，窗口 resize 实时重排；
/// - `bounded`：在 `preferred_line_length`（列数 × em 宽）与编辑器宽度（取小者）处换行。
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum SoftWrapMode {
    None,
    EditorWidth,
    Bounded,
}

impl From<SoftWrapMode> for SoftWrap {
    fn from(mode: SoftWrapMode) -> Self {
        match mode {
            SoftWrapMode::None => SoftWrap::None,
            SoftWrapMode::EditorWidth => SoftWrap::EditorWidth,
            SoftWrapMode::Bounded => SoftWrap::Bounded,
        }
    }
}

/// 字段级容错：该字段值非法时解析为「未配置」（`None`），
/// 由 merge 层用内置默认补齐，不影响其他字段。
/// JSON 语法错误仍整体失败。
///
/// 先解析成 `Value` 再转换：serde_json_lenient 对 enum 字段的非法值走 `peek_error` 路径且不消费 token，直接 `T::deserialize(...).ok()`会让后续字段错位；`Value` 解析总是消费完整 token。
fn fallible<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json_lenient::Value::deserialize(deserializer)?;
    Ok(serde_json_lenient::from_value(value).ok())
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
struct UserSettingsContent {
    #[serde(deserialize_with = "fallible")]
    theme: Option<ThemeContent>,
    #[serde(deserialize_with = "fallible")]
    soft_wrap: Option<SoftWrapMode>,
    #[serde(deserialize_with = "fallible")]
    preferred_line_length: Option<usize>,
    #[serde(deserialize_with = "fallible")]
    file_scan_exclusions: Option<Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UserSettings {
    pub(crate) theme: Theme,
    pub(crate) soft_wrap: SoftWrap,
    pub(crate) preferred_line_length: usize,
    /// 项目树扫描时完全排除的 glob 名单。
    pub(crate) file_scan_exclusions: Vec<String>,
}

/// 解析内置初始设置作为默认层（单一数据源：`initial_user_settings.json`）。
fn default_content() -> UserSettingsContent {
    serde_json_lenient::from_str(INITIAL_USER_SETTINGS).expect("内置初始设置应合法")
}

impl Default for UserSettings {
    fn default() -> Self {
        Self::merge(UserSettingsContent::default())
    }
}

impl UserSettings {
    /// 将用户配置合并到内置默认层：用户显式配置的字段覆盖默认，
    /// 未配置的字段（`None`）回退到内置初始设置。
    fn merge(content: UserSettingsContent) -> Self {
        let defaults = default_content();
        Self {
            theme: content
                .theme
                .or(defaults.theme)
                .map(Theme::from)
                .unwrap_or(Theme::System),
            soft_wrap: content
                .soft_wrap
                .or(defaults.soft_wrap)
                .map(SoftWrap::from)
                .unwrap_or(SoftWrap::None),
            preferred_line_length: content
                .preferred_line_length
                .or(defaults.preferred_line_length)
                .unwrap_or(80),
            file_scan_exclusions: content
                .file_scan_exclusions
                .or(defaults.file_scan_exclusions)
                .unwrap_or_default(),
        }
    }
}

pub(crate) struct SettingsStore {
    settings: UserSettings,
    last_user_settings_content: Option<String>,
    _watcher: Arc<dyn Watcher>,
    _watch_task: Task<()>,
}

impl Global for SettingsStore {}

impl SettingsStore {
    pub(crate) fn get(cx: &App) -> UserSettings {
        cx.global::<Self>().settings.clone()
    }

    /// 读取扫描排除名单；SettingsStore 未初始化（如单元测试）时回退到默认名单。
    pub(crate) fn file_scan_exclusions(cx: &App) -> Vec<String> {
        cx.try_global::<Self>()
            .map(|store| store.settings.file_scan_exclusions.clone())
            .unwrap_or_else(|| UserSettings::default().file_scan_exclusions)
    }

    fn set_user_settings(&mut self, content: &str) -> Result<bool> {
        if self.last_user_settings_content.as_deref() == Some(content) {
            return Ok(false);
        }

        let parsed = parse_user_settings(content)?;
        let settings = UserSettings::merge(parsed);
        self.last_user_settings_content = Some(content.to_owned());
        let changed = settings != self.settings;
        if changed {
            self.settings = settings;
        }
        Ok(changed)
    }
}

pub(crate) fn init(cx: &mut App) {
    let settings_path = crate::paths::settings_file();
    let content = fs::read_to_string(settings_path).unwrap_or_default();
    let mut settings = UserSettings::default();
    let mut last_user_settings_content = None;
    if !content.is_empty() {
        last_user_settings_content = Some(content.clone());
        match parse_user_settings(&content) {
            Ok(parsed) => {
                settings = UserSettings::merge(parsed);
            }
            Err(error) => {
                eprintln!("无法加载设置文件 {}：{error:#}", settings_path.display());
            }
        }
    }
    settings.theme.apply(cx, None);

    let (signal_tx, signal_rx) = async_channel::unbounded::<()>();
    let pending_events = Arc::new(Mutex::new(Vec::new()));
    let watcher: Arc<dyn Watcher> = Arc::new(FsWatcher::new(signal_tx, pending_events.clone()));
    if let Err(error) = watcher.add(crate::paths::config_dir()) {
        log::warn!(
            "无法监听设置目录 {}：{error}",
            crate::paths::config_dir().display()
        );
    }

    let watch_task = cx.spawn(async move |cx| {
        while signal_rx.recv().await.is_ok() {
            // 编辑器保存文件时通常会产生一组连续事件。等待事件安静下来再读取，
            // 避免在 truncate/write 或临时文件替换的中间状态解析设置。
            loop {
                cx.background_executor()
                    .timer(SETTINGS_RELOAD_DEBOUNCE)
                    .await;
                let mut received_more_events = false;
                while signal_rx.try_recv().is_ok() {
                    received_more_events = true;
                }
                if !received_more_events {
                    break;
                }
            }
            std::mem::take(&mut *pending_events.lock().unwrap());

            let settings_path = crate::paths::settings_file();
            let content = match fs::read_to_string(settings_path) {
                Ok(content) => content,
                Err(first_error) => {
                    cx.background_executor()
                        .timer(SETTINGS_RELOAD_RETRY_DELAY)
                        .await;
                    match fs::read_to_string(settings_path) {
                        Ok(content) => content,
                        Err(error) => {
                            eprintln!(
                                "无法读取设置文件 {}：{error}（首次读取错误：{first_error}）",
                                settings_path.display()
                            );
                            continue;
                        }
                    }
                }
            };

            let mut result =
                cx.update_global::<SettingsStore, _>(|store, _| store.set_user_settings(&content));
            if matches!(result, Ok(Err(_))) {
                // 防抖后仍可能撞上非原子写入的极短窗口，再读取一次；真正的配置
                // 错误只在第二次解析仍失败时报告。
                cx.background_executor()
                    .timer(SETTINGS_RELOAD_RETRY_DELAY)
                    .await;
                match fs::read_to_string(settings_path) {
                    Ok(content) => {
                        result = cx.update_global::<SettingsStore, _>(|store, _| {
                            store.set_user_settings(&content)
                        });
                    }
                    Err(error) => {
                        eprintln!("无法读取设置文件 {}：{error}", settings_path.display());
                        continue;
                    }
                }
            }

            match result {
                Ok(Ok(true)) => {
                    let _ = cx.update(|cx| cx.refresh_windows());
                }
                Ok(Ok(false)) => {}
                Ok(Err(error)) => {
                    eprintln!(
                        "无法加载设置文件 {}：{error:#}",
                        crate::paths::settings_file().display()
                    );
                }
                Err(error) => {
                    log::error!("更新设置失败：{error}");
                }
            }
        }
    });

    cx.set_global(SettingsStore {
        settings,
        last_user_settings_content,
        _watcher: watcher,
        _watch_task: watch_task,
    });
}

pub(crate) fn ensure_user_settings_file() -> Result<&'static Path> {
    let path = crate::paths::settings_file();
    let parent = path.parent().context("设置文件缺少父目录")?;
    fs::create_dir_all(parent).with_context(|| format!("无法创建设置目录 {}", parent.display()))?;
    if !path.exists() {
        fs::write(path, INITIAL_USER_SETTINGS)
            .with_context(|| format!("无法创建设置文件 {}", path.display()))?;
    }
    Ok(path)
}

fn parse_user_settings(content: &str) -> Result<UserSettingsContent> {
    if content.trim().is_empty() {
        return Ok(UserSettingsContent::default());
    }
    serde_json_lenient::from_str(content).context("不是合法的 ZCV settings JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_use_defaults() {
        let content = parse_user_settings(r#"{"theme":"one-light"}"#).unwrap();
        let settings = UserSettings::merge(content);
        assert_eq!(settings.theme, Theme::OneLight);
        assert_eq!(settings.soft_wrap, SoftWrap::None);
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
                // 与 Zed 一致，settings.json 使用 JSONC 语义。
                "theme": "one-dark",
                "soft_wrap": "editor-width",
            }"#,
        )
        .unwrap();
        assert_eq!(
            UserSettings::merge(content).soft_wrap,
            SoftWrap::EditorWidth
        );
    }

    #[test]
    fn invalid_field_value_falls_back_to_default() {
        // 非法值字段回退为未配置，由 merge 层用内置默认补齐。
        let settings = UserSettings::merge(parse_user_settings(r#"{"soft_wrap": true}"#).unwrap());
        assert_eq!(settings.soft_wrap, SoftWrap::None);

        let settings = UserSettings::merge(parse_user_settings(r#"{"theme":"unknown"}"#).unwrap());
        assert_eq!(settings.theme, Theme::System);

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
        // 对齐 Zed 的 fallible_options：坏字段单独回退默认，好字段照常生效。
        let settings = UserSettings::merge(
            parse_user_settings(
                r#"{"soft_wrap":"bogus","theme":"one-dark","file_scan_exclusions":["**/target"]}"#,
            )
            .unwrap(),
        );
        assert_eq!(settings.soft_wrap, SoftWrap::None);
        assert_eq!(settings.theme, Theme::OneDark);
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
        assert_eq!(settings.soft_wrap, SoftWrap::Bounded);
        assert_eq!(settings.preferred_line_length, 100);

        let content = parse_user_settings(r#"{"soft_wrap": "editor-width"}"#).unwrap();
        let settings = UserSettings::merge(content);
        assert_eq!(settings.soft_wrap, SoftWrap::EditorWidth);
        assert_eq!(settings.preferred_line_length, 80);

        let content = parse_user_settings(r#"{"soft_wrap": "none"}"#).unwrap();
        assert_eq!(UserSettings::merge(content).soft_wrap, SoftWrap::None);
    }

    #[test]
    fn bundled_initial_settings_are_valid() {
        let content = parse_user_settings(INITIAL_USER_SETTINGS).unwrap();
        assert_eq!(UserSettings::merge(content), UserSettings::default());
    }

    #[test]
    fn invalid_json_reports_location() {
        let error = parse_user_settings(r#"{"theme":}"#).unwrap_err();
        let detailed = format!("{error:#}");
        assert!(detailed.contains("line 1 column"));
    }
}
