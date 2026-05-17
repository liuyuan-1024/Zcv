//! 自绘窗口控制圆点（布局模型 4.2 / 手册 14.6）。
//!
//! 视觉灵感来自 macOS 的三色圆点窗控，但只是我们外壳的设计选型；
//! 这里不感知任何平台语义——动作经由 `shell::platform::window` 统一收口。
//!
//! 交互形态：
//! - 三个圆点组成一个 group；鼠标悬停到 group 内任意位置时，每个圆点
//!   内部浮现自己的符号（× / − / +）。
//! - 圆点本体是可点击按钮，光标为手型；
//!   × 退出整个应用，− 最小化窗口，+ 切换最大化。
//! - 窗口失活时圆点统一变灰，但仍可点击（与 macOS 行为一致）。

use gpui::{Div, FontWeight, Pixels, Rgba, SharedString, Stateful, div, prelude::*, px};

use crate::shell::platform::window as platform_window;
use crate::shell::theme::{color, icon, radius, space};

/// 三个圆点共享的 group 名：用 `group_hover` 让任一悬停都点亮全部符号。
const PIP_GROUP: &str = "top-bar.window-controls";

/// 圆点直径（外径，含边框）——视觉局部常量（手册 3.4）。宽度显式给出
/// 是为了画圆形；高度交给内部符号容器撑开，外框不写死。
fn pip_diameter() -> Pixels {
    icon::i12()
}

/// 边框宽度，1px。`pip_inner_height` 据此推导内容区高度。
const PIP_BORDER: f32 = 1.0;

/// 内容区高度 = 外径 − 上下边框（border-box）。让符号容器的 line_height
/// 取这个值，pip 的最终外径就回到 `pip_diameter()`，保持圆形。
fn pip_inner_height() -> Pixels {
    pip_diameter() - px(PIP_BORDER * 2.0)
}

pub(crate) fn render_window_controls(is_window_active: bool) -> Div {
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
                "\u{00d7}", // ×
            )
            .on_click(|_, _, cx| platform_window::quit(cx)),
        )
        .child(
            control_pip(
                "top-bar.minimize",
                color::control_pip::minimize_fill(),
                color::control_pip::minimize_border(),
                is_window_active,
                "\u{2212}", // −
            )
            .on_click(|_, window, _| platform_window::minimize(window)),
        )
        .child(
            control_pip(
                "top-bar.maximize",
                color::control_pip::maximize_fill(),
                color::control_pip::maximize_border(),
                is_window_active,
                "+",
            )
            .on_click(|_, window, _| platform_window::toggle_maximize(window)),
        )
}

fn control_pip(
    id: &'static str,
    fill: Rgba,
    border: Rgba,
    active: bool,
    symbol: &'static str,
) -> Stateful<Div> {
    let (fill, border) = if active {
        (fill, border)
    } else {
        (
            color::control_pip::inactive_fill(),
            color::control_pip::inactive_border(),
        )
    };

    // 宽度显式给出（圆形需要明确直径），高度不写死——由内部符号容器的
    // line_height 撑开。父 flex 居中安放符号容器。
    div()
        .id(id)
        .w(pip_diameter())
        .rounded(radius::full())
        .bg(fill)
        .border_1()
        .border_color(border)
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .child(pip_symbol(symbol))
}

/// 圆点内部的符号：默认透明，group 悬停时浮现。
///
/// 符号本身用深灰，与彩色圆点保持反差；字号小于直径以留呼吸感；
/// `line_height` 取「内容区高度」，反过来撑开 pip 的高度，使 pip
/// 外径回到圆形所需的 `pip_diameter()`。
fn pip_symbol(symbol: &'static str) -> Div {
    // 符号字号是组件本地常量（手册 3.4）：直径 12px，符号 8px。
    let glyph_size = px(8.0);

    div()
        .flex()
        .items_center()
        .justify_center()
        .text_size(glyph_size)
        .line_height(pip_inner_height())
        .font_weight(FontWeight::BOLD)
        .text_color(color::gray::g00())
        .opacity(0.0)
        .group_hover(PIP_GROUP, |style| style.opacity(1.0))
        .child(SharedString::new_static(symbol))
}
