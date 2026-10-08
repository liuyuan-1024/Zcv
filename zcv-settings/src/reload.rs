//! 设置初始化与文件监听重载。
//!
//! 事件流在 SETTINGS_RELOAD_DEBOUNCE 窗口内合并 notify 事件；
//! 批次命中 settings.json 或配置目录 Rescan 时读取文件并写入 SettingsStore global。
//! 订阅与读取都使用规范化路径，避免 macOS 等后端返回的规范化事件路径无法与前缀匹配。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use gpui::{App, AppContext as _, Global, Task};
use zcv_fs_watch::{FsWatcher, PathEvent, PathEventKind, Watcher};

use super::config_dir;
use super::file::{ensure_user_settings_file, settings_file};
use super::merge::UserSettings;
use super::schema::parse_user_settings;
use super::store::{GlobalSettingsErrorReporter, SettingsErrorReporter, SettingsStore};

const SETTINGS_RELOAD_DEBOUNCE: Duration = Duration::from_millis(75);

/// 应用级设置监听生命周期，与可直接从内存构造的设置值分开。
struct SettingsReload {
    _watcher: Arc<dyn Watcher>,
    _watch_task: Task<()>,
}

impl Global for SettingsReload {}

pub fn init(cx: &mut App) {
    let error_reporter = cx.new(|_| SettingsErrorReporter::default());
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

    let watcher: Arc<dyn Watcher> = Arc::new(FsWatcher::new());
    if let Err(error) = watcher.add(config_dir()) {
        error_reporter.update(cx, |reporter, cx| {
            reporter.report(
                format!("监听设置目录失败（{}）：{error:#}", config_dir().display()),
                cx,
            )
        });
    }

    // 事件路径由 notify 规范化，过滤与读取基准必须同样规范化。
    let watched_dir = canonical_or_original(config_dir());
    let watched_settings = canonical_or_original(settings_file());
    let mut fs_events = watcher.watch(SETTINGS_RELOAD_DEBOUNCE);

    let watch_task = cx.spawn(async move |cx| {
        while let Some(batch) = fs_events.next().await {
            if !batch
                .iter()
                .any(|event| is_settings_event(event, &watched_settings, &watched_dir))
            {
                continue;
            }

            let content = match fs::read_to_string(&watched_settings) {
                Ok(content) => content,
                Err(error) => {
                    error_reporter.update(cx, |reporter, cx| {
                        reporter.report(
                            format!(
                                "读取设置文件失败（{}）：{error:#}",
                                watched_settings.display()
                            ),
                            cx,
                        )
                    });
                    continue;
                }
            };

            match cx.update_global::<SettingsStore, _>(|store, _| store.set_user_settings(&content))
            {
                Ok(true) => {
                    cx.update(|cx| cx.refresh_windows());
                }
                Ok(false) => {}
                Err(error) => {
                    error_reporter.update(cx, |reporter, cx| {
                        reporter.report(format!("更新设置失败：{error:#}"), cx)
                    });
                }
            }
        }
    });

    let mut store = SettingsStore::new(settings);
    store.last_user_settings_content = last_user_settings_content;
    cx.set_global(store);
    cx.set_global(SettingsReload {
        _watcher: watcher,
        _watch_task: watch_task,
    });
}

fn canonical_or_original(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// 只有设置文件本身的变更，或配置目录的 Rescan，才需要重读设置。
///
/// 配置目录同时承载窗口尺寸、工作区布局与更新暂存文件；
/// 不按路径过滤会让这些高频写入反复触发设置解析。
fn is_settings_event(event: &PathEvent, settings_path: &Path, config_dir: &Path) -> bool {
    event.path.as_path() == settings_path
        || (matches!(event.kind, Some(PathEventKind::Rescan)) && event.path.as_path() == config_dir)
}

#[cfg(test)]
#[path = "test/reload_tests.rs"]
mod tests;
