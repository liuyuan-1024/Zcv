//! `workspace.*` 命令目录 —— 项目切换 / 打开。
//!
//! handler 只 emit `HostEffect`（宿主弹项目选择器、走打开流程），不直接碰
//! GPUI / shell。模块名 `workspace` 是内部代号，面向用户文案统一用「项目」。

use crate::{
    CommandArgs, CommandHandler, CommandId, CommandOutcome, CommandRegistry, HostEffect,
    Invocation, Keymap, NoArgs,
};

pub const SHOW_PROJECTS_PICKER: &str = "workspace.show_projects_picker";
pub const OPEN_LOCAL_PROJECT: &str = "workspace.open_local_project";

pub fn open_local_project() -> Invocation {
    (
        CommandId::new(OPEN_LOCAL_PROJECT).expect("内建命令 ID 必须非空"),
        CommandArgs::new(),
    )
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    registry
        .install(
            keymap,
            SHOW_PROJECTS_PICKER,
            "切换项目",
            emit(HostEffect::ShowProjectPicker),
        )
        .key("mod-o");

    registry.install(
        keymap,
        OPEN_LOCAL_PROJECT,
        "打开本地项目",
        emit(HostEffect::OpenLocalProject),
    );
}

/// 与 `window.rs::emit` 同形态；catalog 里"按一个键就推一个 effect"的样板。
fn emit(effect: HostEffect) -> CommandHandler {
    Box::new(move |ctx, args| {
        NoArgs::try_from(args)?;
        ctx.effects.push(effect.clone());
        Ok(CommandOutcome::default())
    })
}