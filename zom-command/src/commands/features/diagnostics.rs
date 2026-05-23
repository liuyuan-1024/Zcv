//! `diagnostics.*` 命令目录。
//!
//! 诊断面板暂未实现；命令先完整注册，状态栏和命令面板可以统一查询元数据。

use crate::{
    CommandArgs, CommandId, CommandOutcome, CommandRegistry, HostEffect, Invocation, Keymap, NoArgs,
};

/// 打开"问题"面板查看诊断列表。
pub const SHOW_PROBLEMS: &str = "diagnostics.show_problems";

pub fn show_problems() -> Invocation {
    (cid(SHOW_PROBLEMS), CommandArgs::new())
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    registry.install(
        keymap,
        SHOW_PROBLEMS,
        "诊断",
        Box::new(|ctx, args| {
            NoArgs::try_from(args)?;
            ctx.effects.push(HostEffect::ShowDiagnostics);
            Ok(CommandOutcome::default())
        }),
    );
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
