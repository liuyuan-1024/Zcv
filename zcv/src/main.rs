#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod active_buffer_language;
mod breadcrumbs;
mod cursor_position;
mod diagnostics;
mod language_tools;
mod paths;
mod preview;
mod project_search;
mod project_tree;
mod recent_projects;
mod settings;
mod version_control;
mod workspace;

use std::path::PathBuf;

use gpui::{
    App, Application, Bounds, Context, Entity, TitlebarOptions, Window, WindowBounds,
    WindowOptions, point, prelude::*, px, size,
};

use workspace::{Dock, Pane, Workspace};
use zcv_assets::Assets;

fn main() {
    Application::new().with_assets(Assets).run(|cx: &mut App| {
        Assets.load_fonts(cx).expect("内置字体应能注册");

        settings::init(cx);
        preview::init(cx);

        // 初始项目根：开发构建打开当前工作区，正式构建打开启动目录。
        #[cfg(debug_assertions)]
        let initial_root = {
            let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            crate_dir.parent().unwrap_or(&crate_dir).to_path_buf()
        };
        #[cfg(not(debug_assertions))]
        let initial_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        open_project_window(initial_root, cx).expect("主窗口应能创建");

        cx.activate(true);
    });
}

/// 打开一个项目窗口（主窗口与「切换项目」的新窗口入口共用）。
pub(crate) fn open_project_window(root: PathBuf, cx: &mut App) -> anyhow::Result<()> {
    recent_projects::add_to_recent(&root.to_string_lossy());

    let bounds = Bounds::centered(None, size(px(1200.0), px(900.0)), cx);

    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("".into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(-100.0), px(-100.0))),
            }),
            ..Default::default()
        },
        |window, cx| {
            settings::SettingsStore::get(cx)
                .theme
                .apply(cx, Some(window));
            let workspace = cx.new(|cx| Workspace::new(root, window, cx));
            workspace.update(cx, |workspace, cx| {
                let pane = workspace.pane.clone();
                initialize_pane(&pane, window, cx);
                // 为每个 Dock 注册焦点转发，与 Zed 做法一致
                for dock in [
                    &workspace.left_dock,
                    &workspace.right_dock,
                    &workspace.bottom_dock,
                ] {
                    dock.update(cx, |dock: &mut Dock, cx: &mut gpui::Context<Dock>| {
                        let focus = dock.focus.clone();
                        let sub = cx.on_focus(
                            &focus,
                            window,
                            |d: &mut Dock, w: &mut gpui::Window, c: &mut gpui::Context<Dock>| {
                                if let Some(panel) = d.visible_panel() {
                                    w.focus(&panel.focus_handle(c));
                                }
                            },
                        );
                        dock._subscriptions.push(sub);
                    });
                }
            });
            window.focus(&workspace.read(cx).focus);
            workspace
        },
    )?;
    Ok(())
}

/// 参照 Zed：在应用层注册 Pane 的 Toolbar 子项。
fn initialize_pane(pane: &Entity<Pane>, window: &mut Window, cx: &mut Context<Workspace>) {
    use crate::breadcrumbs::Breadcrumbs;
    use crate::workspace::FileToolbarControls;

    pane.update(cx, |pane, cx| {
        pane.toolbar().update(cx, |toolbar, cx| {
            toolbar.add_item(cx.new(|_| Breadcrumbs::new()), window, cx);
            toolbar.add_item(cx.new(|_| FileToolbarControls::new()), window, cx);
        });
    });
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use zcv_keymap::load_json;

    /// keymap JSON 引用的所有 action 必须已注册（集成校验：注册来自本 crate 与 zcv-editor）。
    #[gpui::test]
    fn every_platform_keymap_builds_every_registered_action(cx: &mut TestAppContext) {
        cx.update(|cx| {
            for (source, json) in [
                (
                    "default-macos.json",
                    zcv_assets::text("keymaps/default-macos.json")
                        .expect("内置 macOS 快捷键应存在"),
                ),
                (
                    "default-linux.json",
                    zcv_assets::text("keymaps/default-linux.json")
                        .expect("内置 Linux 快捷键应存在"),
                ),
                (
                    "default-windows.json",
                    zcv_assets::text("keymaps/default-windows.json")
                        .expect("内置 Windows 快捷键应存在"),
                ),
            ] {
                let keybindings =
                    load_json(source, &json, cx).expect("每个平台的全部内置绑定都应能构建");
                assert!(!keybindings.bindings.is_empty());
                assert!(
                    cx.build_action("workspace::Save", None).is_ok(),
                    "workspace::Save 应已注册且 keymap 可引用"
                );
            }
        });
    }

    /// 面板切换 action 只能由 dock 持有（防止 status_bar 等重复注册导致快捷键歧义）。
    #[gpui::test]
    fn panel_toggle_actions_are_owned_only_by_dock(cx: &mut TestAppContext) {
        cx.update(|cx| {
            for action in [
                "ToggleProjectTree",
                "ToggleVersionControl",
                "ToggleOutline",
                "ToggleLanguageServer",
                "ToggleDiagnostics",
                "ToggleProjectSearch",
                "ToggleTerminal",
                "ToggleDebug",
                "ToggleKeyboardShortcuts",
            ] {
                assert!(cx.build_action(&format!("dock::{action}"), None).is_ok());
                assert!(
                    cx.build_action(&format!("status_bar::{action}"), None)
                        .is_err()
                );
            }
        });
    }
}
