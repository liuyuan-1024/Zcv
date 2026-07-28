#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod assets;
mod breadcrumbs;
mod diagnostics;
mod editor;
mod fs_watcher;
mod go_to_line;
mod keymap;
mod language_selector;
mod language_tools;
mod languages;
mod project;
mod project_search;
mod project_tree;
mod recent_projects;
mod theme;
mod ui;
mod workspace;

use gpui::{
    App, Application, Bounds, Context, Entity, TitlebarOptions, Window, WindowBounds,
    WindowOptions, point, prelude::*, px, size,
};

use assets::{EmbeddedAssets, embedded_fonts};
use theme::Theme;
use workspace::Workspace;

use crate::workspace::{Dock, Pane};

fn main() {
    Application::new()
        .with_assets(EmbeddedAssets)
        .run(|cx: &mut App| {
            cx.text_system()
                .add_fonts(embedded_fonts())
                .expect("内置字体应能注册");

            Theme::OneDark.apply(None);

            let bounds = Bounds::centered(None, size(px(1200.0), px(900.0)), cx);

            let _window = cx
                .open_window(
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
                        let workspace = cx.new(Workspace::new);
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
                                            |d: &mut Dock,
                                             w: &mut gpui::Window,
                                             c: &mut gpui::Context<Dock>|
                                             {
                                                if let Some(panel) = d.visible_panel() {
                                                    w.focus(&panel.focus_handle(c));
                                                }
                                            },
                                        );
                                    dock._subscriptions.push(sub);
                                });
                            }
                        });
                        #[cfg(debug_assertions)]
                        workspace.update(cx, |workspace, cx| {
                            workspace.open_development_project(window, cx);
                        });
                        window.focus(&workspace.read(cx).focus);
                        workspace
                    },
                )
                .expect("主窗口应能创建");

            cx.activate(true);
        });
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
