//! shell —— GPUI 外壳。
//!
//! 职责（手册 1.2、2.4）：
//! - 视觉层（L1-L4 组件树）
//! - 平台层占位
//! - 启动窗口、装配 `WorkbenchFrame`、提供 `EmbeddedAssetSource`
//!
//! `shell` 不依赖 `app`；本模块只暴露 `run(App)`，由 `main.rs` 调用。

mod assets;
pub(crate) mod components;
pub(crate) mod layout;
pub(crate) mod panel_host;
mod platform;
pub(crate) mod theme;

use gpui::{
    App as GpuiApp, AppContext, Application, Bounds, Context, IntoElement, Render, TitlebarOptions,
    Window, WindowBounds, WindowKind, WindowOptions, point, px, size,
};

use crate::app::App;

use assets::EmbeddedAssets;
use components::regions::workbench_frame;
use layout::WorkbenchState;
use panel_host::PanelHost;

/// 启动桌面应用：装配 GPUI、加载资源、打开首个窗口。
pub fn run(app: App) {
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
                            // 自绘控制按钮
                            appears_transparent: true,
                            traffic_light_position: Some(point(px(-100.0), px(-100.0))),
                        }),
                        kind: WindowKind::Normal,
                        ..Default::default()
                    },
                    |_, cx| cx.new(|_| ShellView::new(app)),
                )
                .expect("GPUI 主窗口应能创建");

            window
                .update(cx, |_, _, cx| {
                    cx.activate(true);
                })
                .expect("GPUI 主窗口应能激活");
        });
}

/// shell 端的根 View：拥有 App 状态与每窗口的 `PanelHost`。
struct ShellView {
    app: App,
    panel_host: PanelHost,
}

impl ShellView {
    fn new(app: App) -> Self {
        Self {
            app,
            panel_host: PanelHost::new(),
        }
    }

    fn workbench_state(&self) -> WorkbenchState {
        self.app.workbench_state()
    }
}

impl Render for ShellView {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.workbench_state();
        workbench_frame::render(&state, &self.panel_host, window)
    }
}
