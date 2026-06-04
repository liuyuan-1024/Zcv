//! `language_server.*` 命令目录。
//!
//! 语言服务器域只表达"当前项目打开了哪些语言服务器、状态如何"；诊断问题列表留在 `diagnostics.*` 域。

use crate::{
    CommandArgs, CommandId, CommandOutcome, CommandRegistry, HostEffect, Invocation,
    KeyBindingContext, Keymap, NoArgs,
};

pub const OPEN_STATUS: &str = "language_server.open_status";
pub const DISMISS: &str = "language_server.dismiss";

/// 语言服务器浮面拥有自己的键盘上下文，Esc 等面板内按键不污染全局快捷键空间。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LanguageServersKeyContext;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LanguageServersBindingContext;

pub fn open_status() -> Invocation {
    (cid(OPEN_STATUS), CommandArgs::new())
}

pub fn dismiss() -> Invocation {
    (cid(DISMISS), CommandArgs::new())
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    let language_servers = KeyBindingContext::language_servers();

    registry
        .install(
            keymap,
            OPEN_STATUS,
            "语言服务器",
            Box::new(|ctx, args| {
                NoArgs::try_from(args)?;
                ctx.effects.push(HostEffect::ShowLanguageServers);
                Ok(CommandOutcome::default())
            }),
        )
        .description("查看当前项目的语言服务器状态。")
        .key("mod-shift-l");

    registry
        .install(
            keymap,
            DISMISS,
            "关闭语言服务器",
            Box::new(|ctx, args| {
                NoArgs::try_from(args)?;
                ctx.effects.push(HostEffect::DismissSurface);
                Ok(CommandOutcome::default())
            }),
        )
        .key_in("escape", language_servers);
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
