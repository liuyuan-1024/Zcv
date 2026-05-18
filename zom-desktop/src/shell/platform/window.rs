//! shell::platform::window —— 窗口控制圆点对应的动作（手册 14.6）。
//!
//! 本模块是「窗控按钮触发的动作」的唯一入口；UI 层（顶栏自绘按钮、命令
//! 系统等）都调用这里，而不是直接戳 gpui。这样一来：
//!
//! - 未来若需要在退出前弹「未保存确认」或落盘 dirty buffer，可以集中
//!   在这里加，不必散落到各处按钮。
//! - 各 OS 的差异（例如 Linux 上 zoom 的行为）也只在这里处理一次。

use gpui::{App, Window};

/// 窗口命令执行后由 shell 应用到当前 GPUI 窗口的动作。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WindowAction {
    Quit,
    Minimize,
    ToggleMaximize,
}

pub(crate) fn apply(action: WindowAction, window: &Window, cx: &App) {
    match action {
        WindowAction::Quit => quit(cx),
        WindowAction::Minimize => minimize(window),
        WindowAction::ToggleMaximize => toggle_maximize(window),
    }
}

/// 退出整个应用——× 圆点对应的动作。
///
/// 注意：第一版直接走 gpui 的 `quit`；二期接入退出协议（手册 25.x）
/// 后再加确认 / 落盘等步骤。
pub(crate) fn quit(cx: &App) {
    cx.quit();
}

/// 最小化当前窗口。
pub(crate) fn minimize(window: &Window) {
    window.minimize_window();
}

/// 切换最大化/还原（macOS 上是 zoom）。
pub(crate) fn toggle_maximize(window: &Window) {
    window.zoom_window();
}
