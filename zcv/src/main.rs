#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod active_buffer_language;
mod auto_update;
mod breadcrumbs;
mod cursor_position;
mod harness;
mod project_diff;
mod version_control;
mod workspace;

use std::ffi::OsString;
use std::path::PathBuf;

use gpui::{App, Application};
use workspace::{open_empty_workspace, open_project_window};
use zcv_assets::Assets;
use zcv_settings::SettingsStore;
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
    Application::new()
        .with_assets(Assets)
        .with_http_client(http_client)
        .run(|cx: &mut App| {
            Assets.load_fonts(cx).expect("内置字体应能注册");

            zcv_settings::init(cx);
            // 字号设置落地：SettingsStore 已就绪，按配置覆盖主题默认字号。
            {
                let settings = SettingsStore::get(cx);
                zcv_theme::typography::set_typography(
                    Some(settings.font_size),
                    Some(settings.ui_font_size),
                    Some(settings.line_height),
                );
            }
            zcv_preview_markdown::init(cx);
            zcv_preview_svg::init(cx);
            zcv_editor::init(cx);
            zcv_keymap::init(cx).expect("内置快捷键应能注册");
            auto_update::init(cx);

            match initial_project_root(std::env::args_os(), most_recent_valid_project()) {
                Some(root) => {
                    // 打开失败（路径已失效等）回退空工作区，不阻塞启动。
                    if let Err(error) = open_project_window(root, cx) {
                        eprintln!("打开项目失败：{error}");
                        open_empty_workspace(cx).expect("空工作区窗口应能创建");
                    }
                }
                None => {
                    open_empty_workspace(cx).expect("空工作区窗口应能创建");
                }
            }

            if let Err(error) = auto_update::acknowledge_started_update() {
                eprintln!("无法确认新版本启动：{error:#}");
            }
            cx.activate(true);
        });
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use gpui::TestAppContext;

    use super::initial_project_root;

    #[test]
    fn startup_uses_explicit_command_line_project() {
        assert_eq!(
            initial_project_root(
                [OsString::from("zcv"), OsString::from("/project")].into_iter(),
                Some(PathBuf::from("/recent")),
            ),
            Some(PathBuf::from("/project"))
        );
    }

    #[test]
    fn startup_without_path_uses_recent_project_or_none() {
        assert_eq!(
            initial_project_root(
                [OsString::from("zcv")].into_iter(),
                Some(PathBuf::from("/recent")),
            ),
            Some(PathBuf::from("/recent"))
        );
        assert_eq!(
            initial_project_root([OsString::from("zcv")].into_iter(), None),
            None
        );
    }

    /// Panel 键盘命令使用单一带参 Action，不为每个 Panel 声明类型。
    #[gpui::test]
    fn panel_keyboard_action_uses_stable_panel_id(cx: &mut TestAppContext) {
        cx.update(|cx| {
            for action in ["ToggleLeftDock", "ToggleBottomDock", "ToggleRightDock"] {
                assert!(cx.build_action(&format!("dock::{action}"), None).is_ok());
            }
            let action = cx
                .build_action(
                    "dock::FocusOrHidePanel",
                    Some(serde_json::json!({ "panel": "project-tree" })),
                )
                .expect("带稳定 Panel ID 的通用 Action 应能构建");
            assert_eq!(action.name(), "dock::FocusOrHidePanel");
        });
    }
}
