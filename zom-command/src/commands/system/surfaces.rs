//! `surface.*` 命令目录。
//!
//! handler 只 emit [`HostEffect`]，具体关闭哪个浮面由宿主自己的
//! `SurfaceManager` 决定；命令系统不持有 shell 状态。

use crate::{
    CommandArgs, CommandId, CommandOutcome, CommandRegistry, HostEffect, Invocation, Keymap, NoArgs,
};

pub const DISMISS: &str = "surface.dismiss";

pub fn dismiss() -> Invocation {
    (cid(DISMISS), CommandArgs::new())
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    registry
        .install(
            keymap,
            DISMISS,
            "关闭浮面",
            Box::new(|ctx, args| {
                NoArgs::try_from(args)?;
                ctx.effects.push(HostEffect::DismissSurface);
                Ok(CommandOutcome::default())
            }),
        )
        .key("escape");
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
