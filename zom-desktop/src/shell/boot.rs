//! GPUI 应用启动与首窗口装配。

use gpui::{
    App as GpuiApp, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowKind,
    WindowOptions, point, px, size,
};

use crate::app::App;

use super::assets::EmbeddedAssets;
use super::platform::app_icon;
use super::view::ShellView;

/// 启动桌面应用：装配 GPUI、加载资源、打开首个窗口。
pub fn run(app: App) {
    app_icon::prepare_development_app_icon();

    Application::new()
        .with_assets(EmbeddedAssets)
        .run(move |cx: &mut GpuiApp| {
            let bounds = Bounds::centered(None, size(px(960.0), px(640.0)), cx);

            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some("zom".into()),
                            // 自绘窗控圆点（手册 14.6）：让 OS 标题栏透明，并把
                            // 系统自带的圆点挪到屏外，由 `top_bar::window_controls`
                            // 自绘控制按钮。
                            appears_transparent: true,
                            traffic_light_position: Some(point(px(-100.0), px(-100.0))),
                        }),
                        kind: WindowKind::Normal,
                        app_id: Some(app_icon::APP_ID.to_string()),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| ShellView::new(app, cx)),
                )
                .expect("GPUI 主窗口应能创建");

            window
                .update(cx, |shell_view, window, cx| {
                    cx.activate(true);
                    window.focus(&shell_view.editor_focus());
                })
                .expect("GPUI 主窗口应能激活");

            window
                .update(cx, |_, window, _| app_icon::apply_window_icon(window))
                .expect("GPUI 主窗口应能设置开发态图标");
        });
}
