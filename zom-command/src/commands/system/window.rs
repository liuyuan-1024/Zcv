//! `window.*` 命令目录 —— 退出 / 最小化 / 最大化。
//!
//! handler 不直接操作窗口 —— 接触 GPUI 会让 zom-command 反向依赖平台层。
//! 取而代之，emit [`HostEffect`]，宿主翻译成具体 API 调用。

use crate::commands::emit;
use crate::{CommandArgs, CommandId, CommandRegistry, HostEffect, Invocation, Keymap};

pub const QUIT: &str = "window.quit";
pub const MINIMIZE: &str = "window.minimize";
pub const TOGGLE_MAXIMIZE: &str = "window.toggle_maximize";

pub fn quit() -> Invocation {
    (cid(QUIT), CommandArgs::new())
}

pub fn minimize() -> Invocation {
    (cid(MINIMIZE), CommandArgs::new())
}

pub fn toggle_maximize() -> Invocation {
    (cid(TOGGLE_MAXIMIZE), CommandArgs::new())
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    registry
        .install(keymap, QUIT, "退出应用", emit(HostEffect::Quit))
        .description("退出应用。")
        .key("mod-q");
    registry
        .install(keymap, MINIMIZE, "最小化窗口", emit(HostEffect::Minimize))
        .description("最小化当前窗口。")
        .key("mod-m");
    registry
        .install(
            keymap,
            TOGGLE_MAXIMIZE,
            "切换窗口最大化",
            emit(HostEffect::ToggleMaximize),
        )
        .description("在普通窗口和最大化窗口之间切换。")
        .key("mod-shift-m");
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
