#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod auto_update;
mod harness;
mod workspace;

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, Application};
use workspace::{open_empty_workspace, open_empty_workspace_with_error, open_project_window};
use zcv_assets::Assets;
use zcv_language::LanguageRegistry;
use zcv_settings::SettingsStore;
use zcv_theme::typography;
use zcv_workspace::most_recent_valid_project;

fn initial_project_root(
    mut args: impl Iterator<Item = OsString>,
    recent_project: Option<PathBuf>,
) -> Option<PathBuf> {
    // args 首项是可执行名，第二项起是命令行路径
    args.nth(1).map(PathBuf::from).or(recent_project)
}

fn main() {
    let http_client = auto_update::new_http_client().expect("无法初始化自动更新 HTTP 客户端");
    Application::with_platform(gpui_platform::current_platform(false))
        .with_assets(Assets)
        .with_http_client(http_client)
        .run(|cx: &mut App| {
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
            let languages = Arc::new(LanguageRegistry::new());
            zcv_preview_markdown::init(Arc::clone(&languages), cx);
            zcv_preview_svg::init(cx);
            zcv_editor::init(cx);
            zcv_preview_image::init(cx);
            zcv_search::init(cx);
            zcv_version_control::init(cx);
            zcv_keymap::init(cx).expect("内置快捷键应能注册");
            auto_update::init(cx);

            // 更新 helper 需要在有限时间内确认新版本已经启动；
            // 项目恢复和窗口创建可能较慢，不能把它们放在启动确认之前。
            if let Err(error) = auto_update::acknowledge_started_update() {
                eprintln!("无法确认新版本启动：{error:#}");
            }

            match initial_project_root(std::env::args_os(), most_recent_valid_project()) {
                Some(root) => {
                    // 打开失败（路径已失效等）回退空工作区，不阻塞启动。
                    if let Err(error) = open_project_window(root, Arc::clone(&languages), cx) {
                        open_empty_workspace_with_error(
                            format!("打开项目失败：{error:#}"),
                            languages,
                            cx,
                        )
                        .expect("空工作区窗口应能创建");
                    }
                }
                None => {
                    open_empty_workspace(languages, cx).expect("空工作区窗口应能创建");
                }
            }

            cx.activate(true);
        });
}

#[cfg(test)]
#[path = "test/main_tests.rs"]
mod tests;
