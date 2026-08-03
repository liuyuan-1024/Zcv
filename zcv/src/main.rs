#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod assets;
mod breadcrumbs;
mod diagnostics;
mod fs_watcher;
mod go_to_line;
mod keymap;
mod language_selector;
mod language_tools;
mod paths;
mod project;
mod project_search;
mod project_tree;
mod recent_projects;
mod settings;
mod ui;
mod workspace;

use std::path::PathBuf;

use gpui::{
    App, Application, Bounds, Context, Entity, TitlebarOptions, Window, WindowBounds,
    WindowOptions, point, prelude::*, px, size,
};

use assets::{EmbeddedAssets, embedded_fonts};
use workspace::{Dock, Pane, Workspace};

fn main() {
    Application::new()
        .with_assets(EmbeddedAssets)
        .run(|cx: &mut App| {
            cx.text_system()
                .add_fonts(embedded_fonts())
                .expect("内置字体应能注册");

            settings::init(cx);

            // 初始项目根：开发构建打开 zcv 工作区本身，正式构建打开启动目录。
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

/// 打开一个项目窗口（主窗口与未来的「新窗口打开」入口共用）。
fn open_project_window(root: PathBuf, cx: &mut App) -> anyhow::Result<()> {
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

    pane.update(cx, |pane, cx| {
        pane.toolbar().update(cx, |toolbar, cx| {
            toolbar.add_item(cx.new(|_| Breadcrumbs::new()), window, cx);
        });
    });
}
