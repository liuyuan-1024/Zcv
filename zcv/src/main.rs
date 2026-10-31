#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

mod app_menus;
mod auto_update;
mod harness;
mod open_with;
#[cfg(target_os = "windows")]
mod single_instance;
mod workspace;

use std::{ffi::OsString, path::PathBuf, sync::Arc};

use futures::{StreamExt, channel::mpsc};
use gpui::{App, Application};
use workspace::{
    default_file_watcher_factory, handle_open_paths, open_empty_workspace,
    open_empty_workspace_with_error, open_project_window,
};
use zcv_assets::Assets;
use zcv_language::LanguageRegistry;
use zcv_settings::SettingsStore;
use zcv_theme::typography;
use zcv_workspace::most_recent_valid_project;

#[derive(Debug, PartialEq, Eq)]
enum InitialOpen {
    Project(PathBuf),
    File(PathBuf),
    Empty,
}

fn initial_open(
    mut args: impl Iterator<Item = OsString>,
    recent_project: Option<PathBuf>,
) -> InitialOpen {
    let path = args.nth(1).map(PathBuf::from).or(recent_project);
    match path {
        Some(path) if path.is_file() => InitialOpen::File(path),
        Some(path) => InitialOpen::Project(path),
        None => InitialOpen::Empty,
    }
}

fn open_initial(
    initial: InitialOpen,
    languages: Arc<LanguageRegistry>,
    watcher_factory: workspace::FileWatcherFactory,
    cx: &mut App,
) {
    match initial {
        InitialOpen::Project(root) => {
            if let Err(error) = open_project_window(
                root,
                Arc::clone(&languages),
                Arc::clone(&watcher_factory),
                cx,
            ) {
                open_empty_workspace_with_error(
                    format!("打开项目失败：{error:#}"),
                    languages,
                    watcher_factory,
                    cx,
                )
                .expect("空工作区窗口应能创建");
            }
        }
        InitialOpen::File(path) => {
            handle_open_paths(vec![path], vec![], languages, watcher_factory, cx);
        }
        InitialOpen::Empty => {
            open_empty_workspace(languages, watcher_factory, cx).expect("空工作区窗口应能创建");
        }
    }
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    #[cfg(target_os = "windows")]
    let request = initial_open(args.iter().cloned(), None);
    let initial = initial_open(args.into_iter(), most_recent_valid_project());
    #[cfg(target_os = "windows")]
    let instance = if single_instance::enforce_single_instance(
        cfg!(debug_assertions),
        std::env::var_os("ZCV_STATELESS").is_some(),
    ) {
        match single_instance::start(&request) {
            Ok(Some(instance)) => Some(instance),
            Ok(None) => return,
            Err(error) => panic!("转交文件打开请求失败：{error:#}"),
        }
    } else {
        // 开发构建允许新旧二进制并存：不创建互斥体，也不接管文件打开请求。
        None
    };

    open_with::ensure_open_with_registration();

    let http_client = auto_update::new_http_client();
    let update_error = http_client
        .as_ref()
        .err()
        .map(|error| format!("自动更新不可用：{error:#}"));
    let mut application =
        Application::with_platform(gpui_platform::current_platform(false)).with_assets(Assets);
    if let Ok(http_client) = http_client {
        application = application.with_http_client(http_client);
    }
    let languages = Arc::new(LanguageRegistry::new());
    let watcher_factory = default_file_watcher_factory();
    let (open_urls_tx, mut open_urls_rx) = mpsc::unbounded::<Vec<String>>();
    #[cfg(target_os = "windows")]
    let (instance_tx, mut instance_rx) = mpsc::unbounded::<InitialOpen>();
    #[cfg(target_os = "windows")]
    if let Some(instance) = &instance {
        instance.listen(instance_tx);
    }
    application.on_open_urls(move |urls| {
        open_urls_tx
            .unbounded_send(urls)
            .expect("文件打开事件接收任务应保持运行");
    });
    application.on_reopen({
        let languages = Arc::clone(&languages);
        let watcher_factory = Arc::clone(&watcher_factory);
        move |cx| {
            open_initial(
                initial_open(std::iter::empty(), most_recent_valid_project()),
                Arc::clone(&languages),
                Arc::clone(&watcher_factory),
                cx,
            );
            cx.activate(true);
        }
    });
    application.run(move |cx: &mut App| {
        Assets.load_fonts(cx).expect("内置字体应能注册");

        zcv_settings::init(cx);
        // 排版设置落地：SettingsStore 已就绪，按配置覆盖主题默认字号。
        {
            let settings = SettingsStore::get(cx);
            typography::set_base_typography(
                cx,
                Some(settings.content_font_size),
                Some(settings.ui_font_size),
                Some(settings.content_line_height),
            );
        }
        // 应用级唯一语言注册表：预览 Provider 与工作区内的 Project 共享同一份。
        zcv_preview_markdown::init(Arc::clone(&languages), cx);
        zcv_preview_svg::init(cx);
        zcv_editor::init(cx, Arc::clone(&languages));
        zcv_preview_image::init(cx);
        zcv_search::init(cx);
        zcv_version_control::init(cx);
        zcv_keymap::init(cx).expect("内置快捷键应能注册");
        workspace::install_open_local_file_action(cx);
        workspace::install_open_local_directory_action(Arc::clone(&languages), cx);
        app_menus::install(cx);
        auto_update::init(cx, update_error);

        // 更新 helper 需要在有限时间内确认新版本已经启动；
        // 项目恢复和窗口创建可能较慢，不能把它们放在启动确认之前。
        if let Err(error) = auto_update::acknowledge_started_update() {
            eprintln!("无法确认新版本启动：{error:#}");
        }

        open_initial(
            initial,
            Arc::clone(&languages),
            Arc::clone(&watcher_factory),
            cx,
        );

        #[cfg(target_os = "windows")]
        cx.spawn({
            let languages = Arc::clone(&languages);
            let watcher_factory = Arc::clone(&watcher_factory);
            async move |cx| {
                while let Some(request) = instance_rx.next().await {
                    cx.update(|cx| match request {
                        InitialOpen::Empty => {
                            if let Some(window) = cx
                                .active_window()
                                .or_else(|| cx.windows().into_iter().next())
                            {
                                window
                                    .update(cx, |_, window, _| window.activate_window())
                                    .ok();
                            }
                            cx.activate(true);
                        }
                        request => open_initial(
                            request,
                            Arc::clone(&languages),
                            Arc::clone(&watcher_factory),
                            cx,
                        ),
                    });
                }
            }
        })
        .detach();

        cx.spawn({
            let languages = Arc::clone(&languages);
            let watcher_factory = Arc::clone(&watcher_factory);
            async move |cx| {
                while let Some(urls) = open_urls_rx.next().await {
                    cx.update(|cx| {
                        workspace::handle_open_urls(
                            urls,
                            Arc::clone(&languages),
                            Arc::clone(&watcher_factory),
                            cx,
                        )
                    });
                }
            }
        })
        .detach();

        cx.activate(true);
    });
}

#[cfg(test)]
#[path = "test/support.rs"]
mod test_support;

#[cfg(test)]
#[path = "test/main_tests.rs"]
mod tests;
