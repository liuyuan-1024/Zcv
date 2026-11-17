//! 应用级设置命令与独立窗口装配。

use std::{sync::Arc, time::Duration};

use gpui::{App, AppContext as _, Focusable as _, PromptLevel};
use zcv_actions::{OpenSettings, OpenSettingsFile};
use zcv_language::LanguageRegistry;
use zcv_settings_window::SettingsWindow;
use zcv_ui::ToastKind;

use crate::workspace::{self, FileWatcherFactory};

pub(crate) fn install(
    cx: &mut App,
    languages: Arc<LanguageRegistry>,
    watcher_factory: FileWatcherFactory,
) {
    cx.on_action(|_: &OpenSettings, cx| {
        cx.defer(|cx| {
            if let Err(error) = open(cx) {
                report_open_error(format!("打开设置窗口失败：{error:#}"), cx);
            }
        });
    });
    cx.on_action(move |_: &OpenSettingsFile, cx| {
        let languages = Arc::clone(&languages);
        let watcher_factory = Arc::clone(&watcher_factory);
        cx.defer(move |cx| {
            let result = zcv_settings::ensure_user_settings_file().and_then(|path| {
                workspace::open_paths_in_workspace(
                    vec![path],
                    vec![],
                    languages,
                    watcher_factory,
                    cx,
                )
            });
            if let Err(error) = result {
                report_open_error(format!("打开设置文件失败：{error:#}"), cx);
            }
        });
    });
}

fn open(cx: &mut App) -> anyhow::Result<()> {
    if let Some(existing) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<SettingsWindow>())
    {
        existing.update(cx, |_, window, _| window.activate_window())?;
        return Ok(());
    }

    let display_id = match cx.active_window() {
        Some(active) => active.update(cx, |_, window, cx| {
            window.display(cx).map(|display| display.id())
        })?,
        None => None,
    };
    cx.open_window(SettingsWindow::options(display_id, cx), |window, cx| {
        crate::appearance::observe_window(window);
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        let focus = settings.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        settings
    })?;
    Ok(())
}

fn report_open_error(message: String, cx: &mut App) {
    if let Some(settings) = cx
        .active_window()
        .and_then(|window| window.downcast::<SettingsWindow>())
        && settings
            .update(cx, |settings, _, cx| {
                settings.show_error(message.clone(), cx)
            })
            .is_ok()
    {
        return;
    }
    if let Some(window) = workspace::active_workspace_window(cx)
        && window
            .update(cx, |workspace, _, cx| {
                workspace.show_toast(
                    ToastKind::Error,
                    message.clone(),
                    vec![],
                    Some(Duration::from_secs(8)),
                    cx,
                );
            })
            .is_ok()
    {
        return;
    }
    // 没有可承载提示层的窗口时，仍通过系统提示报告窗口创建失败。
    if let Some(window) = cx.active_window() {
        let _ = window.update(cx, |_, window, cx| {
            let prompt = window.prompt(PromptLevel::Critical, &message, None, &["知道了"], cx);
            workspace::retain_prompt_until_answer(prompt, cx);
        });
    } else {
        eprintln!("{message}");
    }
}
