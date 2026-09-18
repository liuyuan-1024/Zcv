//! 设置初始化与文件监听重载。
//!
//! 防抖合并连续文件事件，重试非原子写入窗口，解析结果写入 `SettingsStore` global。

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use gpui::{App, AppContext as _};
use zcv_fs_watch::{FsWatcher, Watcher};

use super::config_dir;
use super::file::{ensure_user_settings_file, settings_file};
use super::merge::UserSettings;
use super::schema::parse_user_settings;
use super::store::{GlobalSettingsErrorReporter, SettingsErrorReporter, SettingsStore};

const SETTINGS_RELOAD_DEBOUNCE: Duration = Duration::from_millis(75);
const SETTINGS_RELOAD_RETRY_DELAY: Duration = Duration::from_millis(50);

pub fn init(cx: &mut App) {
    let error_reporter = cx.new(|_| SettingsErrorReporter::new());
    cx.set_global(GlobalSettingsErrorReporter(error_reporter.clone()));
    let settings_path = settings_file();
    if let Err(error) = ensure_user_settings_file() {
        error_reporter.update(cx, |reporter, cx| {
            reporter.report(
                format!(
                    "初始化用户设置文件失败（{}）：{error:#}",
                    settings_path.display()
                ),
                cx,
            )
        });
    }
    let content = match fs::read_to_string(settings_path) {
        Ok(content) => content,
        Err(error) => {
            error_reporter.update(cx, |reporter, cx| {
                reporter.report(
                    format!("读取设置文件失败（{}）：{error:#}", settings_path.display()),
                    cx,
                )
            });
            String::new()
        }
    };
    let mut settings = UserSettings::default();
    let mut last_user_settings_content = None;
    if !content.is_empty() {
        last_user_settings_content = Some(content.clone());
        match parse_user_settings(&content) {
            Ok(parsed) => {
                settings = UserSettings::merge(parsed);
            }
            Err(error) => {
                error_reporter.update(cx, |reporter, cx| {
                    reporter.report(
                        format!("加载设置文件失败（{}）：{error:#}", settings_path.display()),
                        cx,
                    )
                });
            }
        }
    }
    let watcher = Arc::new(FsWatcher::new());
    let fs_events = watcher.events();
    let watcher: Arc<dyn Watcher> = watcher;
    if let Err(error) = watcher.add(config_dir()) {
        error_reporter.update(cx, |reporter, cx| {
            reporter.report(
                format!("监听设置目录失败（{}）：{error:#}", config_dir().display()),
                cx,
            )
        });
    }

    let watch_task = cx.spawn(async move |cx| {
        while fs_events.next_batch().await.is_some() {
            // 编辑器保存文件时通常会产生一组连续事件。
            // 等待事件安静下来再读取，避免在 truncate/write 或临时文件替换的中间状态解析设置。
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
                            let message = format!(
                                "读取设置文件失败（{}）：{error:#}（首次读取错误：{first_error:#}）",
                                settings_path.display()
                            );
                            error_reporter.update(cx, |reporter, cx| reporter.report(message, cx));
                            continue;
                        }
                    }
                }
            };

            let mut result =
                cx.update_global::<SettingsStore, _>(|store, _| store.set_user_settings(&content));
            if matches!(result, Ok(false)) {
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
                        let message = format!(
                            "读取设置文件失败（{}）：{error:#}",
                            settings_path.display()
                        );
                        error_reporter.update(cx, |reporter, cx| reporter.report(message, cx));
                        continue;
                    }
                }
            }

            match result {
                Ok(true) => {
                    cx.update(|cx| cx.refresh_windows());
                }
                Ok(false) => {}
                Err(error) => {
                    let message = format!("更新设置失败：{error:#}");
                    error_reporter.update(cx, |reporter, cx| reporter.report(message, cx));
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
