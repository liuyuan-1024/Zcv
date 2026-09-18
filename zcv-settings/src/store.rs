//! 设置的运行时存储与错误上报。
//!
//! `SettingsStore` 是用户设置 global 的唯一载体；错误经 `SettingsErrorReporter` 通知装配层。

use std::sync::Arc;

use anyhow::Result;
use gpui::{App, Context, Entity, EventEmitter, Global, Task};
use zcv_fs_watch::Watcher;

use super::merge::UserSettings;
use super::schema::parse_user_settings;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsError(pub String);

pub struct SettingsErrorReporter {
    pending: Option<String>,
}

impl EventEmitter<SettingsError> for SettingsErrorReporter {}

pub struct GlobalSettingsErrorReporter(pub Entity<SettingsErrorReporter>);

impl Global for GlobalSettingsErrorReporter {}

impl SettingsErrorReporter {
    pub(crate) fn new() -> Self {
        Self { pending: None }
    }

    pub fn take_pending(&mut self) -> Option<String> {
        self.pending.take()
    }

    pub(crate) fn report(&mut self, message: String, cx: &mut Context<Self>) {
        self.pending = Some(message.clone());
        cx.emit(SettingsError(message));
    }
}

pub struct SettingsStore {
    pub(crate) settings: UserSettings,
    pub(crate) last_user_settings_content: Option<String>,
    pub(crate) _watcher: Arc<dyn Watcher>,
    pub(crate) _watch_task: Task<()>,
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

    pub(crate) fn set_user_settings(&mut self, content: &str) -> Result<bool> {
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
