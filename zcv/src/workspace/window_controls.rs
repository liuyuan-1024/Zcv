//! 窗口控制 —— 自绘 macOS 风格三色圆点。

use gpui::{Pixels, Window, actions, div, prelude::*, px, rgb, svg};

use zcv_theme::{color, space};

actions!(
    window_controls,
    [QuitWindow, MinimizeWindow, ToggleMaximizeWindow,]
);

const PIP_GROUP: &str = "window-controls.pips";

// ── 按键经 action 路由至此（点击不走这里，在 render 中直调）────

pub(crate) fn handle_quit(_: &QuitWindow, _: &mut Window, cx: &mut gpui::App) {
    cx.quit();
}
pub(crate) fn handle_minimize(_: &MinimizeWindow, window: &mut Window, _: &mut gpui::App) {
    window.minimize_window();
}
pub(crate) fn handle_toggle_maximize(
    _: &ToggleMaximizeWindow,
    window: &mut Window,
    _: &mut gpui::App,
) {
    window.zoom_window();
}

pub(crate) fn render(window: &Window, cx: &gpui::App) -> gpui::Stateful<gpui::Div> {
    let active = window.is_window_active();

    div()
        .id("window-controls")
        .group(PIP_GROUP)
        .flex()
        .flex_row()
        .items_center()
        .gap(space::S8)
        .child(
            pip(Pip::Close, active, cx)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(QuitWindow), cx)),
        )
        .child(
            pip(Pip::Minimize, active, cx)
                .on_click(|_, window, cx| window.dispatch_action(Box::new(MinimizeWindow), cx)),
        )
        .child(
            pip(Pip::Maximize, active, cx).on_click(|_, window, cx| {
                window.dispatch_action(Box::new(ToggleMaximizeWindow), cx)
            }),
        )
}

// ── 私有渲染辅助函数 ─────────────────────────────────────────────────

fn pip(pip: Pip, active: bool, cx: &gpui::App) -> gpui::Stateful<gpui::Div> {
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
        .rounded(Pixels::MAX)
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
                .text_color(color::current(cx).icon_on_accent)
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
}
