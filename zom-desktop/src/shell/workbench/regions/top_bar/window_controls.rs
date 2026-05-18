//! 自绘窗口控制圆点（布局模型 4.2 / 手册 14.6）。
//!
//! 视觉灵感来自 macOS 的三色圆点窗控，但只是我们外壳的设计选型；
//! 这里不直接操作窗口——动作先派发为命令，再由组合根转换成窗口动作。
//!
//! 交互形态：
//! - 三个圆点组成一个 group；鼠标悬停到 group 内任意位置时，每个圆点
//!   内部浮现自己的符号（× / − / +）。
//! - 圆点本体是可点击按钮，光标为手型；
//!   × 退出整个应用，− 最小化窗口，+ 切换最大化。
//! - 窗口失活时圆点统一变灰，但仍可点击（与 macOS 行为一致）。

use gpui::{Div, Rgba, Stateful, Svg, div, prelude::*, svg};

use crate::shell::WindowControlsHandlers;
use crate::shell::shared::theme::{color, icon, radius, space};

/// 三个圆点共享的 group 名：用 `group_hover` 让任一悬停都点亮全部符号。
const PIP_GROUP: &str = "top-bar.window-controls";

const CLOSE_SYMBOL: &str = "icons/top_bar/window_controls/close.svg";
const MINIMIZE_SYMBOL: &str = "icons/top_bar/window_controls/minimize.svg";
const MAXIMIZE_SYMBOL: &str = "icons/top_bar/window_controls/maximize.svg";

pub(crate) fn render_window_controls(
    is_window_active: bool,
    handlers: WindowControlsHandlers,
) -> Div {
    let WindowControlsHandlers {
        quit,
        minimize,
        toggle_maximize,
    } = handlers;

    div()
        .group(PIP_GROUP)
        .flex()
        .flex_row()
        .items_center()
        .gap(space::s8())
        .child(
            control_pip(
                "top-bar.close",
                color::control_pip::close_fill(),
                color::control_pip::close_border(),
                is_window_active,
                CLOSE_SYMBOL,
            )
            .on_click(move |_, window, cx| quit(window, cx)),
        )
        .child(
            control_pip(
                "top-bar.minimize",
                color::control_pip::minimize_fill(),
                color::control_pip::minimize_border(),
                is_window_active,
                MINIMIZE_SYMBOL,
            )
            .on_click(move |_, window, cx| minimize(window, cx)),
        )
        .child(
            control_pip(
                "top-bar.maximize",
                color::control_pip::maximize_fill(),
                color::control_pip::maximize_border(),
                is_window_active,
                MAXIMIZE_SYMBOL,
            )
            .on_click(move |_, window, cx| toggle_maximize(window, cx)),
        )
}

fn control_pip(
    id: &'static str,
    fill: Rgba,
    border: Rgba,
    active: bool,
    symbol_path: &'static str,
) -> Stateful<Div> {
    let (fill, border) = if active {
        (fill, border)
    } else {
        (
            color::control_pip::inactive_fill(),
            color::control_pip::inactive_border(),
        )
    };

    div()
        .id(id)
        .w(icon::i12())
        .h(icon::i12())
        .rounded(radius::full())
        .bg(fill)
        .border_1()
        .border_color(border)
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .child(pip_symbol(symbol_path))
}

/// 圆点内部的符号：默认透明，group 悬停时浮现。
///
/// 符号本身用深灰，与彩色圆点保持反差；尺寸小于直径以留呼吸感。
fn pip_symbol(path: &'static str) -> Svg {
    svg()
        .path(path)
        .size(icon::i10())
        .text_color(color::gray::g00())
        .opacity(0.0)
        .group_hover(PIP_GROUP, |style| style.opacity(1.0))
}
