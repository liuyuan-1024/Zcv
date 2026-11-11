//! 设置的运行时存储与错误上报。
//!
//! `SettingsStore` 是用户设置 global 的唯一载体；错误经 `SettingsErrorReporter` 通知装配层。

use std::path::Path;

use anyhow::Result;
use gpui::{App, BorrowAppContext as _, Context, Entity, EventEmitter, Global};

use super::{
    file::{
        read_user_settings_file_at, serialize_user_settings, settings_file,
        write_user_settings_file_at,
    },
    merge::UserSettings,
    schema::{UserSettingsContent, parse_user_settings, validate_user_settings},
};

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
    /// 最近一次成功加载的用户覆盖值；面板读取它判断哪些字段被显式填写。
    pub(crate) user_content: UserSettingsContent,
    pub(crate) last_user_settings_content: Option<String>,
}

impl Global for SettingsStore {}

impl SettingsStore {
    /// 从已解析的设置创建运行时存储；文件监听由初始化模块独立持有。
    pub fn new(settings: UserSettings) -> Self {
        Self {
            settings,
            user_content: UserSettingsContent::default(),
            last_user_settings_content: None,
        }
    }

    pub fn get(cx: &App) -> UserSettings {
        cx.global::<Self>().settings.clone()
    }

    /// 读取用户显式填写的覆盖值；未填写的字段为 `None`。
    pub fn user_settings_content(cx: &App) -> UserSettingsContent {
        cx.global::<Self>().user_content.clone()
    }

    /// 按引用读取渲染热路径使用的标量设置；Store 必须已在应用初始化时注册。
    pub fn minimum_contrast_for_highlights(cx: &App) -> f32 {
        cx.global::<Self>().settings.minimum_contrast_for_highlights
    }

    /// 读取扫描排除名单；Store 必须已在应用初始化时注册。
    pub fn file_scan_exclusions(cx: &App) -> Vec<String> {
        cx.global::<Self>().settings.file_scan_exclusions.clone()
    }

    pub fn set_user_settings(&mut self, content: &str) -> Result<bool> {
        if self.last_user_settings_content.as_deref() == Some(content) {
            return Ok(false);
        }

        let parsed = parse_user_settings(content)?;
        Ok(self.apply_user_settings(parsed, content))
    }

    fn apply_user_settings(&mut self, content: UserSettingsContent, text: &str) -> bool {
        let settings = UserSettings::merge(content.clone());
        let changed = settings != self.settings;
        self.user_content = content;
        self.last_user_settings_content = Some(text.to_owned());
        if changed {
            self.settings = settings;
        }
        changed
    }
}

/// 更新用户设置文件并立即应用：面板与手工编辑共用同一解析、校验和生效流程。
///
/// 先读取并解析当前文件，修改覆盖内容后再校验与序列化；写入成功后才替换
/// `SettingsStore` 快照。写入失败时文件与运行时设置都保持原状。
pub fn update_user_settings(
    cx: &mut App,
    update: impl FnOnce(&mut UserSettingsContent),
) -> Result<bool> {
    update_user_settings_at(settings_file(), cx, update)
}

pub(crate) fn update_user_settings_at(
    path: &Path,
    cx: &mut App,
    update: impl FnOnce(&mut UserSettingsContent),
) -> Result<bool> {
    let old_text = read_user_settings_file_at(path)?;
    let mut content = parse_user_settings(&old_text)?;
    update(&mut content);
    validate_user_settings(&content)?;
    let new_text = serialize_user_settings(&content)?;
    if new_text == old_text {
        return Ok(false);
    }
    write_user_settings_file_at(path, &new_text)?;
    let mut changed = false;
    cx.update_global::<SettingsStore, _>(|store, _| {
        changed = store.apply_user_settings(content, &new_text);
    });
    Ok(changed)
}
