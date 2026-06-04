//! `settings.*` 命令目录。
//!
//! 设置界面暂未实现；命令先完整注册，宿主收到 effect 后决定展示占位或忽略。

use crate::{
    CommandArgs, CommandId, CommandOutcome, CommandRegistry, HostEffect, Invocation,
    KeyBindingContext, Keymap, NoArgs,
};

/// 打开设置面板。
pub const OPEN: &str = "settings.open";
/// 关闭设置面板。
pub const DISMISS: &str = "settings.dismiss";

/// 设置面板拥有自己的键盘上下文，Esc 等面板内按键不污染全局快捷键空间。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SettingsKeyContext;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SettingsBindingContext;

pub fn open() -> Invocation {
    (cid(OPEN), CommandArgs::new())
}

pub fn dismiss() -> Invocation {
    (cid(DISMISS), CommandArgs::new())
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    let settings = KeyBindingContext::settings();

    registry
        .install(
            keymap,
            OPEN,
            "设置",
            Box::new(|ctx, args| {
                NoArgs::try_from(args)?;
                ctx.effects.push(HostEffect::ShowSettings);
                Ok(CommandOutcome::default())
            }),
        )
        .description("打开设置面板。")
        .key("mod-,");

    registry
        .install(
            keymap,
            DISMISS,
            "关闭设置",
            Box::new(|ctx, args| {
                NoArgs::try_from(args)?;
                ctx.effects.push(HostEffect::DismissSurface);
                Ok(CommandOutcome::default())
            }),
        )
        .key_in("escape", settings);
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
