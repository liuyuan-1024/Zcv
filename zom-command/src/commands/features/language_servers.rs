//! `language_server.*` 命令目录。
//!
//! 语言服务器域只表达"当前项目打开了哪些语言服务器、状态如何"；诊断
//! 问题列表留在 `diagnostics.*` 域。

use crate::{
    CommandArgs, CommandId, CommandOutcome, CommandRegistry, HostEffect, Invocation, Keymap, NoArgs,
};

pub const OPEN_STATUS: &str = "language_server.open_status";

pub fn open_status() -> Invocation {
    (cid(OPEN_STATUS), CommandArgs::new())
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
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
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
