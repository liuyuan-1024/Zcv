//! 设置的运行时存储与错误上报。
//!
//! `SettingsStore` 是用户设置 global 的唯一载体；错误经 `SettingsErrorReporter` 通知装配层。

use anyhow::Result;
use gpui::{App, Context, Entity, EventEmitter, Global};

use super::merge::UserSettings;
use super::schema::parse_user_settings;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsError(pub String);

#[derive(Default)]
pub struct SettingsErrorReporter {
    pending: Option<String>,
}

impl EventEmitter<SettingsError> for SettingsErrorReporter {}

pub struct GlobalSettingsErrorReporter(pub Entity<SettingsErrorReporter>);

impl Global for GlobalSettingsErrorReporter {}

impl SettingsErrorReporter {
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
}

impl Global for SettingsStore {}

impl SettingsStore {
    /// 从已解析的设置创建运行时存储；文件监听由初始化模块独立持有。
    pub fn new(settings: UserSettings) -> Self {
        Self {
            settings,
            last_user_settings_content: None,
        }
    }

    pub fn get(cx: &App) -> UserSettings {
        cx.global::<Self>().settings.clone()
    }

    /// 设置未注册时返回 None，消费方回退默认值。
    pub fn try_get(cx: &App) -> Option<UserSettings> {
        cx.try_global::<Self>().map(|store| store.settings.clone())
    }

    /// 渲染热路径按引用读取标量设置；未注册 Store 时由调用方决定默认策略。
    pub fn minimum_contrast_for_highlights(cx: &App) -> Option<f32> {
        cx.try_global::<Self>()
            .map(|store| store.settings.minimum_contrast_for_highlights)
    }

    /// 读取扫描排除名单；SettingsStore 未初始化（如单元测试）时回退到默认名单。
    pub fn file_scan_exclusions(cx: &App) -> Vec<String> {
        cx.try_global::<Self>()
            .map(|store| store.settings.file_scan_exclusions.clone())
            .unwrap_or_else(|| UserSettings::default().file_scan_exclusions)
    }

    pub fn set_user_settings(&mut self, content: &str) -> Result<bool> {
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
