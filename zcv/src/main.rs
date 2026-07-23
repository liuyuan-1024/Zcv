#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod editor;
mod features;
mod keymap;
mod shared;
mod theme;
mod workbench;

use gpui::{
    App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, prelude::*, px,
    size,
};

use shared::assets::{EmbeddedAssets, embedded_fonts};
use theme::Theme;
use workbench::Workspace;

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
                    |_, cx| cx.new(Workspace::new),
                )
                .expect("主窗口应能创建");
        });
}
