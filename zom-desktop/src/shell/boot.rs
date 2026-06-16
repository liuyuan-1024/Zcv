//! GPUI 应用启动与首窗口装配。

use gpui::{
    App as GpuiApp, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowKind,
    WindowOptions, point, px, size,
};

use crate::app::App;

use super::platform::app_icon;
use super::shared::assets::{EmbeddedAssets, embedded_fonts};
use super::view::ShellView;

/// 启动桌面应用：装配 GPUI、加载资源、打开首个窗口。
pub fn run(app: App) {
    app_icon::prepare_development_app_icon();

    Application::new()
        .with_assets(EmbeddedAssets)
        .run(move |cx: &mut GpuiApp| {
            cx.text_system()
                .add_fonts(embedded_fonts())
                .expect("内置字体应能注册到 GPUI text system");

            let bounds = Bounds::centered(None, size(px(1200.0), px(900.0)), cx);

            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some("zom".into()),
                            // 自绘窗控圆点（手册 14.6）：让 OS 标题栏透明，并把系统自带的圆点挪到屏外。
                            // 由 `top_bar::window_controls` 自绘控制按钮。
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
                    shell_view.install_feature_listeners(window, cx);
                    shell_view.flush_startup_bubbles(window, cx);

                    // 开发阶段：启动即打开固定项目，方便测试。
                    // 走与选择器完全相同的打开流程；release 构建不编译此分支。
                    #[cfg(debug_assertions)]
                    shell_view.open_project(
                        std::path::PathBuf::from("/Users/liuyuan/project/liuyuan/zom"),
                        window,
                        cx,
                    );
                })
                .expect("GPUI 主窗口应能激活");

            window
                .update(cx, |_, window, _| app_icon::apply_window_icon(window))
                .expect("GPUI 主窗口应能设置开发态图标");
        });
}
