//! `settings.*` 命令目录。
//!
//! 设置界面暂未实现；命令先完整注册，宿主收到 effect 后决定展示占位或忽略。

use crate::{
    CommandArgs, CommandId, CommandOutcome, CommandRegistry, HostEffect, Invocation, Keymap, NoArgs,
};

/// 打开设置面板。
pub const OPEN: &str = "settings.open";

pub fn open() -> Invocation {
    (cid(OPEN), CommandArgs::new())
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
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
        .key("mod-,");
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
