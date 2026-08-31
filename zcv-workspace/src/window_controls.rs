//! 窗口控制 —— 自绘 macOS 风格三色圆点。

use gpui::{Window, div, prelude::*, px, rgb, svg};
pub(crate) use zcv_actions::{MinimizeWindow, QuitWindow, ToggleMaximizeWindow};
use zcv_theme::space;

const PIP_GROUP: &str = "window-controls.pips";

// ── 窗口 action handler；Quit 由 Workspace 在刷新布局状态后处理 ──

pub(crate) fn handle_minimize(_: &MinimizeWindow, window: &mut Window, _: &mut gpui::App) {
    window.minimize_window();
}
pub(crate) fn render(window: &Window) -> gpui::Stateful<gpui::Div> {
    let active = window.is_window_active();

    div()
        .id("window-controls")
        // 左侧补 S2：与右侧设置按钮（Compact 内边距 S2）视觉对齐，两侧内容到窗口边界等距。
        .pl(space::S2)
        .group(PIP_GROUP)
        .flex()
        .flex_row()
        .items_center()
        .gap(space::S8)
        .child(
            pip(Pip::Close, active)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(QuitWindow), cx)),
        )
        .child(
            pip(Pip::Minimize, active)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(MinimizeWindow), cx)),
        )
        .child(
            pip(Pip::Maximize, active).on_click(|_, window, cx| {
                window.dispatch_action(Box::new(ToggleMaximizeWindow), cx)
            }),
        )
}

// ── 私有渲染辅助函数 ─────────────────────────────────────────────────

fn pip(pip: Pip, active: bool) -> gpui::Stateful<gpui::Div> {
    let id = match pip {
        Pip::Close => "window-controls.close",
        Pip::Minimize => "window-controls.minimize",
        Pip::Maximize => "window-controls.maximize",
    };
    let (fill, border) = pip.palette(active);
    let (hover_fill, hover_border) = pip.palette(true);

    div()
        .id(id)
        .size(px(12.0))
        .rounded_full()
        .bg(fill)
        .border_1()
        .border_color(border)
        .group_hover(PIP_GROUP, |style| {
            if !active {
                style.bg(hover_fill).border_color(hover_border)
            } else {
                style
            }
        })
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .child(
            svg()
                .path(pip.svg_path())
                .size(px(10.0))
                // 符号为该按钮色的深色调。
                .text_color(pip.symbol_color())
                .opacity(0.0)
                .group_hover(PIP_GROUP, |style| style.opacity(1.0)),
        )
}

// ── 内部类型 ─────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum Pip {
    Close,
    Minimize,
    Maximize,
}

impl Pip {
    fn palette(self, active: bool) -> (gpui::Rgba, gpui::Rgba) {
        if !active {
            return (rgb(0x6f7378), rgb(0x5c6066));
        }
        match self {
            Pip::Close => (rgb(0xff5f57), rgb(0xe0443e)),
            Pip::Minimize => (rgb(0xffbd2e), rgb(0xde9f18)),
            Pip::Maximize => (rgb(0x28c840), rgb(0x1aab29)),
        }
    }

    fn svg_path(self) -> &'static str {
        match self {
            Pip::Close => "icons/generic_close.svg",
            Pip::Minimize => "icons/minimize.svg",
            Pip::Maximize => "icons/maximize.svg",
        }
    }

    /// 悬停符号色（近似 macOS 交通灯：按钮色的深色调，红按钮深红褐、黄按钮深黄褐、绿按钮深绿）。
    fn symbol_color(self) -> gpui::Rgba {
        match self {
            Pip::Close => rgb(0x460804),
            Pip::Minimize => rgb(0x90591d),
            Pip::Maximize => rgb(0x2a6218),
        }
    }
}
