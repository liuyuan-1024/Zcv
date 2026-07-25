#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod assets;
mod editor;
mod keymap;
mod languages;
mod theme;
mod ui;
mod workspace;

use gpui::{
    App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, prelude::*, px,
    size,
};

use assets::{EmbeddedAssets, embedded_fonts};
use theme::Theme;
use workspace::Workspace;

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
                        // 注册 Pane 焦点监听，使 StatusBar 能跟随 Pane 切换
                        workspace.update(cx, |workspace, cx| {
                            if let Some(pane) = workspace.focus_pane.clone() {
                                workspace.register_pane_focus_listener(&pane, window, cx);
                            }
                            // 为每个 Dock 注册焦点转发，与 Zed 做法一致
                            for dock in [
                                &workspace.left_dock,
                                &workspace.right_dock,
                                &workspace.bottom_dock,
                            ] {
                                dock.update(
                                    cx,
                                    |dock: &mut workspace::dock::Dock,
                                     cx: &mut gpui::Context<workspace::dock::Dock>|
                                     {
                                        let focus = dock.focus.clone();
                                        let sub = cx.on_focus(
                                            &focus,
                                            window,
                                            |d: &mut workspace::dock::Dock,
                                             w: &mut gpui::Window,
                                             c: &mut gpui::Context<workspace::dock::Dock>|
                                             {
                                                if let Some(panel) = d.visible_panel() {
                                                    w.focus(&panel.focus_handle(c));
                                                }
                                            },
                                        );
                                        dock._subscriptions.push(sub);
                                    },
                                );
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
