//! 应用级设置窗口装配：单实例、窗口尺寸与错误提示。

use std::sync::Arc;

use gpui::{
    App, AppContext as _, Focusable as _, PromptLevel, TitlebarOptions, WindowBounds,
    WindowOptions, px, size,
};
use zcv_actions::{OpenSettings, OpenSettingsFile};
use zcv_language::LanguageRegistry;
use zcv_settings_window::SettingsWindow;

pub(crate) fn install(cx: &mut App, languages: Arc<LanguageRegistry>) {
    let languages_for_file = Arc::clone(&languages);
    cx.on_action(move |_: &OpenSettings, cx| {
        if let Err(error) = open(cx, &languages, false) {
            report_open_error(cx, error);
        }
    });
    cx.on_action(move |_: &OpenSettingsFile, cx| {
        if let Err(error) = open(cx, &languages_for_file, true) {
            report_open_error(cx, error);
        }
    });
}

fn open(cx: &mut App, languages: &Arc<LanguageRegistry>, show_file: bool) -> anyhow::Result<()> {
    if let Some(existing) = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<SettingsWindow>())
    {
        existing.update(cx, |settings, window, cx| {
            settings.show_view(show_file, window, cx);
            window.activate_window();
        })?;
        return Ok(());
    }

    let languages = Arc::clone(languages);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(960.0), px(720.0)), cx)),
            titlebar: Some(TitlebarOptions {
                title: Some("Zcv — 设置".into()),
                // 与主窗口一致：透明标题栏，由窗口内容自绘标题栏并设置红绿灯位置。
                appears_transparent: true,
                traffic_light_position: None,
            }),
            focus: true,
            show: true,
            window_min_size: Some(size(px(680.0), px(520.0))),
            ..Default::default()
        },
        move |window, cx| {
            let settings = cx.new(|cx| SettingsWindow::new(languages, show_file, window, cx));
            let focus = settings.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
            settings
        },
    )
    .map_err(|error| anyhow::anyhow!("无法创建设置窗口：{error}"))?;
    Ok(())
}

fn report_open_error(cx: &mut App, error: anyhow::Error) {
    let message = format!("打开设置窗口失败：{error:#}");
    let Some(active_window) = cx.active_window() else {
        eprintln!("{message}");
        return;
    };
    let result: anyhow::Result<()> = active_window.update(cx, |_, window, cx| {
        let prompt = window.prompt(
            PromptLevel::Critical,
            "打开设置窗口失败",
            Some(&message),
            &["知道了"],
            cx,
        );
        crate::workspace::retain_prompt_until_answer(prompt, cx);
    });
    if let Err(error) = result {
        eprintln!("{message}：{error:#}");
    }
}
