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
pub const START_GIT_CLONE: &str = "workspace.start_git_clone";
pub const REMOVE_RECENT_PROJECT: &str = "workspace.remove_recent_project";

pub fn show_projects_picker() -> Invocation {
    (cid(SHOW_PROJECTS_PICKER), CommandArgs::new())
}

pub fn open_local_project() -> Invocation {
    (cid(OPEN_LOCAL_PROJECT), CommandArgs::new())
}

pub fn start_git_clone() -> Invocation {
    (cid(START_GIT_CLONE), CommandArgs::new())
}

pub fn remove_recent_project() -> Invocation {
    (cid(REMOVE_RECENT_PROJECT), CommandArgs::new())
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

    registry
        .install(
            keymap,
            OPEN_LOCAL_PROJECT,
            "从本地路径导入",
            emit(HostEffect::OpenLocalProject),
        )
        .key("mod-l");

    registry
        .install(
            keymap,
            START_GIT_CLONE,
            "从 Git 地址导入",
            emit(HostEffect::StartGitClone),
        )
        .key("mod-g");

    registry
        .install(
            keymap,
            REMOVE_RECENT_PROJECT,
            "移除最近项目",
            emit(HostEffect::RemoveSelectedRecentProject),
        )
        .key("mod-backspace");
}

/// 与 `window.rs::emit` 同形态；catalog 里"按一个键就推一个 effect"的样板。
fn emit(effect: HostEffect) -> CommandHandler {
    Box::new(move |ctx, args| {
        NoArgs::try_from(args)?;
        ctx.effects.push(effect.clone());
        Ok(CommandOutcome::default())
    })
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
