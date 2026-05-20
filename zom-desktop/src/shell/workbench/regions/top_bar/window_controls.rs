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

use gpui::{Div, Pixels, Rgba, Stateful, Svg, div, prelude::*, px, rgb, svg};

use crate::shell::WindowControlsHandlers;
use crate::shell::shared::theme::{color, radius, space};

/// 三个圆点的身份。
///
/// 配色（填充 + 描边）随枚举走：灵感来自 macOS 三色窗控，但只是本组件的设计
/// 选型——不携带平台语义、也拒绝主题改写（手册 3.4），因此不进 `theme`。
#[derive(Clone, Copy)]
enum Pip {
    Close,
    Minimize,
    Maximize,
}

impl Pip {
    /// 窗口活跃时该圆点的配色 `(填充, 描边)`。
    fn active_palette(self) -> (Rgba, Rgba) {
        match self {
            Pip::Close => (rgb(0xff5f57), rgb(0xe0443e)),
            Pip::Minimize => (rgb(0xffbd2e), rgb(0xde9f18)),
            Pip::Maximize => (rgb(0x28c840), rgb(0x1aab29)),
        }
    }
}

/// 窗口失活时三个圆点统一变灰，共用这组 `(填充, 描边)`。
fn inactive_palette() -> (Rgba, Rgba) {
    (rgb(0x6f7378), rgb(0x5c6066))
}

/// 三个圆点共享的 group 名：用 `group_hover` 让任一悬停都点亮全部符号。
const PIP_GROUP: &str = "top-bar.window-controls";

const CLOSE_SYMBOL: &str = "icons/top_bar/window_controls/close.svg";
const MINIMIZE_SYMBOL: &str = "icons/top_bar/window_controls/minimize.svg";
const MAXIMIZE_SYMBOL: &str = "icons/top_bar/window_controls/maximize.svg";

/// 圆点直径。圆点是窗控的本地几何尺寸，不是「UI 图标」，故不走 `theme::icon`。
const PIP_SIZE: Pixels = px(12.0);
/// 圆点内符号尺寸，略小于直径留呼吸感。
const PIP_SYMBOL_SIZE: Pixels = px(10.0);

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
            control_pip("top-bar.close", Pip::Close, is_window_active, CLOSE_SYMBOL)
                .on_click(move |_, window, cx| quit(window, cx)),
        )
        .child(
            control_pip(
                "top-bar.minimize",
                Pip::Minimize,
                is_window_active,
                MINIMIZE_SYMBOL,
            )
            .on_click(move |_, window, cx| minimize(window, cx)),
        )
        .child(
            control_pip(
                "top-bar.maximize",
                Pip::Maximize,
                is_window_active,
                MAXIMIZE_SYMBOL,
            )
            .on_click(move |_, window, cx| toggle_maximize(window, cx)),
        )
}

fn control_pip(
    id: &'static str,
    pip: Pip,
    active: bool,
    symbol_path: &'static str,
) -> Stateful<Div> {
    let (fill, border) = if active {
        pip.active_palette()
    } else {
        inactive_palette()
    };

    div()
        .id(id)
        .size(PIP_SIZE)
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
        .size(PIP_SYMBOL_SIZE)
        .text_color(color::gray::g00())
        .opacity(0.0)
        .group_hover(PIP_GROUP, |style| style.opacity(1.0))
}
