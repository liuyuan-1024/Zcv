//! 设置系统：用户设置文件的解析、合并与变更监听。
//! 此文件是 `zcv-settings` crate 的公共入口。
//!
//! 用户设置 JSON 经 fs watcher 监听，变更防抖重载后写入 [`SettingsStore`] global；
//! 默认值与各领域设置由本模块统一提供；具体的运行时类型转换由消费方完成。

use std::borrow::Cow;
use std::fs;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use anyhow::{Context as _, Result};
use gpui::{App, Global, Task};
use serde::Deserialize;
use zcv_fs_watch::{FsWatcher, Watcher};

/// 配置目录与设置文件路径解析（用户目录 → 配置目录 → 设置文件）。
pub fn config_dir() -> &'static Path {
    static CONFIG_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    CONFIG_DIR.get_or_init(|| home_dir().join(".zcv")).as_path()
}

fn settings_file() -> &'static Path {
    static SETTINGS_FILE: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    SETTINGS_FILE
        .get_or_init(|| config_dir().join("settings.json"))
        .as_path()
}

fn home_dir() -> std::path::PathBuf {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");

    home.map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()))
}

static INITIAL_USER_SETTINGS: LazyLock<Cow<'static, str>> = LazyLock::new(|| {
    zcv_assets::text("settings/initial_user_settings.json").expect("内置初始设置应存在")
});
const SETTINGS_RELOAD_DEBOUNCE: Duration = Duration::from_millis(75);
const SETTINGS_RELOAD_RETRY_DELAY: Duration = Duration::from_millis(50);

/// 软换行模式的设置值，语义与 Zed 的 `soft_wrap` 一致。
///
/// - `none`：不换行，超长行靠水平滚动查看；
/// - `editor-width`：行宽超过编辑器文本区宽度时换行，窗口 resize 实时重排；
/// - `bounded`：在 `preferred_line_length`（列数 × em 宽）与编辑器宽度（取小者）处换行。
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SoftWrapMode {
    None,
    EditorWidth,
    Bounded,
}

/// 字段级容错：该字段值非法时解析为「未配置」（`None`），由 merge 层用内置默认补齐，不影响其他字段。
/// JSON 语法错误仍整体失败。
///
/// 先解析成 `Value` 再转换：serde_json_lenient 对 enum 字段的非法值走 `peek_error` 路径且不消费 token，直接 `T::deserialize(...).ok()`会让后续字段错位；
/// `Value` 解析总是消费完整 token。
fn fallible<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json_lenient::Value::deserialize(deserializer)?;
    Ok(serde_json_lenient::from_value(value).ok())
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default)]
struct UserSettingsContent {
    #[serde(deserialize_with = "fallible")]
    theme: Option<String>,
    #[serde(deserialize_with = "fallible")]
    font_size: Option<f32>,
    #[serde(deserialize_with = "fallible")]
    ui_font_size: Option<f32>,
    #[serde(deserialize_with = "fallible")]
    line_height: Option<f32>,
    #[serde(deserialize_with = "fallible")]
    soft_wrap: Option<SoftWrapMode>,
    #[serde(deserialize_with = "fallible")]
    preferred_line_length: Option<usize>,
    #[serde(deserialize_with = "fallible")]
    file_scan_exclusions: Option<Vec<String>>,
    #[serde(deserialize_with = "fallible")]
    use_autoclose: Option<bool>,
    #[serde(deserialize_with = "fallible")]
    use_auto_surround: Option<bool>,
    #[serde(deserialize_with = "fallible")]
    terminal_font_size: Option<f32>,
    #[serde(deserialize_with = "fallible")]
    terminal_line_height: Option<f32>,
    #[serde(deserialize_with = "fallible")]
    terminal_max_scroll_history_lines: Option<usize>,
    #[serde(deserialize_with = "fallible")]
    terminal_cursor_shape: Option<String>,
    #[serde(deserialize_with = "fallible")]
    terminal_alternate_scroll: Option<bool>,
    #[serde(deserialize_with = "fallible")]
    terminal_option_as_meta: Option<bool>,
    #[serde(deserialize_with = "fallible")]
    terminal_shell: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UserSettings {
    /// 主题配置 id；由主题模块解析为运行时主题。
    pub theme: String,
    /// 编辑器字号（像素）。
    pub font_size: f32,
    /// UI 字号（像素）。
    pub ui_font_size: f32,
    /// 编辑器行高（相对字号的倍数）。
    pub line_height: f32,
    pub soft_wrap: SoftWrapMode,
    /// 软换行的目标行宽（列数）；仅在 `soft_wrap = "bounded"` 时生效。
    pub preferred_line_length: usize,
    /// 项目树扫描时完全排除的 glob 名单。
    pub file_scan_exclusions: Vec<String>,
    /// 键入配对起始字符时是否自动补全闭合符。
    pub use_autoclose: bool,
    /// 选中文本时键入配对起始字符是否用该对包裹选区。
    pub use_auto_surround: bool,
    /// 终端字体大小（像素）；缺省时跟随编辑器字号。
    pub terminal_font_size: Option<f32>,
    /// 终端行高（相对字号的倍数）；缺省时跟随编辑器行高。
    pub terminal_line_height: Option<f32>,
    /// 终端滚动回看上限行数。
    pub terminal_max_scroll_history_lines: usize,
    /// 终端光标形状："block" | "underline" | "bar"。
    pub terminal_cursor_shape: String,
    /// 备用屏幕下滚轮是否转发为方向键。
    pub terminal_alternate_scroll: bool,
    /// Option 键是否作为 Meta 键使用。
    pub terminal_option_as_meta: bool,
    /// 终端 shell 程序；缺省时使用系统默认 shell。
    pub terminal_shell: Option<String>,
}

/// 解析内置初始设置作为默认层，保证默认值只有一个数据源。
fn default_content() -> UserSettingsContent {
    serde_json_lenient::from_str(&INITIAL_USER_SETTINGS).expect("内置初始设置应合法")
}

impl Default for UserSettings {
    fn default() -> Self {
        Self::merge(UserSettingsContent::default())
    }
}

impl UserSettings {
    /// 将用户配置合并到内置默认层：用户显式配置的字段覆盖默认，未配置的字段（`None`）回退到内置初始设置。
    fn merge(content: UserSettingsContent) -> Self {
        let defaults = default_content();
        // 默认值唯一数据源是内置 initial_user_settings.json。
        Self {
            theme: content.theme.or(defaults.theme).expect("内置默认应存在"),
            font_size: content
                .font_size
                .or(defaults.font_size)
                .expect("内置默认应存在"),
            ui_font_size: content
                .ui_font_size
                .or(defaults.ui_font_size)
                .expect("内置默认应存在"),
            line_height: content
                .line_height
                .or(defaults.line_height)
                .expect("内置默认应存在"),
            soft_wrap: content
                .soft_wrap
                .or(defaults.soft_wrap)
                .expect("内置默认应存在"),
            preferred_line_length: content
                .preferred_line_length
                .or(defaults.preferred_line_length)
                .expect("内置默认应存在"),
            file_scan_exclusions: content
                .file_scan_exclusions
                .or(defaults.file_scan_exclusions)
                .expect("内置默认应存在"),
            use_autoclose: content
                .use_autoclose
                .or(defaults.use_autoclose)
                .expect("内置默认应存在"),
            use_auto_surround: content
                .use_auto_surround
                .or(defaults.use_auto_surround)
                .expect("内置默认应存在"),
            terminal_font_size: content.terminal_font_size.or(defaults.terminal_font_size),
            terminal_line_height: content
                .terminal_line_height
                .or(defaults.terminal_line_height),
            terminal_max_scroll_history_lines: content
                .terminal_max_scroll_history_lines
                .or(defaults.terminal_max_scroll_history_lines)
                .expect("内置默认应存在"),
            terminal_cursor_shape: content
                .terminal_cursor_shape
                .or(defaults.terminal_cursor_shape)
                .expect("内置默认应存在"),
            terminal_alternate_scroll: content
                .terminal_alternate_scroll
                .or(defaults.terminal_alternate_scroll)
                .expect("内置默认应存在"),
            terminal_option_as_meta: content
                .terminal_option_as_meta
                .or(defaults.terminal_option_as_meta)
                .expect("内置默认应存在"),
            terminal_shell: content.terminal_shell.or(defaults.terminal_shell),
        }
    }
}

pub struct SettingsStore {
    settings: UserSettings,
    last_user_settings_content: Option<String>,
    _watcher: Arc<dyn Watcher>,
    _watch_task: Task<()>,
}

impl Global for SettingsStore {}

impl SettingsStore {
    pub fn get(cx: &App) -> UserSettings {
        cx.global::<Self>().settings.clone()
    }

    /// 设置未注册时返回 None，消费方回退默认值。
    pub fn try_get(cx: &App) -> Option<UserSettings> {
        cx.try_global::<Self>().map(|store| store.settings.clone())
    }

    /// 读取扫描排除名单；SettingsStore 未初始化（如单元测试）时回退到默认名单。
    pub fn file_scan_exclusions(cx: &App) -> Vec<String> {
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

pub fn init(cx: &mut App) {
    let settings_path = settings_file();
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
    let watcher = Arc::new(FsWatcher::new());
    let fs_events = watcher.events();
    let watcher: Arc<dyn Watcher> = watcher;
    if let Err(error) = watcher.add(config_dir()) {
        eprintln!("无法监听设置目录 {}：{error}", config_dir().display());
    }

    let watch_task = cx.spawn(async move |cx| {
        while fs_events.next_batch().await.is_some() {
            // 编辑器保存文件时通常会产生一组连续事件。等待事件安静下来再读取，
            // 避免在 truncate/write 或临时文件替换的中间状态解析设置。
            loop {
                cx.background_executor()
                    .timer(SETTINGS_RELOAD_DEBOUNCE)
                    .await;
                if !fs_events.has_more() {
                    break;
                }
            }

            let settings_path = settings_file();
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
                    eprintln!("无法加载设置文件 {}：{error:#}", settings_file().display());
                }
                Err(error) => {
                    eprintln!("更新设置失败：{error}");
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

pub fn ensure_user_settings_file() -> Result<&'static Path> {
    let path = settings_file();
    let parent = path.parent().context("设置文件缺少父目录")?;
    fs::create_dir_all(parent).with_context(|| format!("无法创建设置目录 {}", parent.display()))?;
    if !path.exists() {
        fs::write(path, INITIAL_USER_SETTINGS.as_bytes())
            .with_context(|| format!("无法创建设置文件 {}", path.display()))?;
    }
    Ok(path)
}

fn parse_user_settings(content: &str) -> Result<UserSettingsContent> {
    if content.trim().is_empty() {
        return Ok(UserSettingsContent::default());
    }
    serde_json_lenient::from_str(content).context("不是合法的 settings JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // 对齐 Zed 的 fallible_options：坏字段单独回退默认，好字段照常生效。
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
