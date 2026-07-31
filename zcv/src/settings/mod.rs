use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result};
use gpui::{App, Global, Task};
use serde::Deserialize;
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

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
struct UserSettingsContent {
    theme: Option<ThemeContent>,
    soft_wrap: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UserSettings {
    pub(crate) theme: Theme,
    pub(crate) soft_wrap: bool,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            theme: Theme::System,
            soft_wrap: false,
        }
    }
}

impl UserSettings {
    fn merge(content: UserSettingsContent) -> Self {
        let defaults = Self::default();
        Self {
            theme: content.theme.map(Theme::from).unwrap_or(defaults.theme),
            soft_wrap: content.soft_wrap.unwrap_or(defaults.soft_wrap),
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
        cx.global::<Self>().settings
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
    settings.theme.apply(None);

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
        assert_eq!(
            UserSettings::merge(content),
            UserSettings {
                theme: Theme::OneLight,
                soft_wrap: false,
            }
        );
    }

    #[test]
    fn comments_and_trailing_commas_are_supported() {
        let content = parse_user_settings(
            r#"{
                // 与 Zed 一致，settings.json 使用 JSONC 语义。
                "theme": "one-dark",
                "soft_wrap": true,
            }"#,
        )
        .unwrap();
        assert!(UserSettings::merge(content).soft_wrap);
    }

    #[test]
    fn bundled_initial_settings_are_valid() {
        let content = parse_user_settings(INITIAL_USER_SETTINGS).unwrap();
        assert_eq!(UserSettings::merge(content), UserSettings::default());
    }

    #[test]
    fn invalid_theme_is_rejected() {
        let error = parse_user_settings(r#"{"theme":"unknown"}"#).unwrap_err();
        assert!(error.to_string().contains("不是合法的 ZCV settings JSON"));
        assert!(format!("{error:#}").contains("unknown variant"));
    }

    #[test]
    fn invalid_json_reports_location() {
        let error = parse_user_settings(r#"{"theme":}"#).unwrap_err();
        let detailed = format!("{error:#}");
        assert!(detailed.contains("line 1 column"));
    }
}
