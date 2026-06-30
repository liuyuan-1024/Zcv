//! `workspace.*` 命令目录 —— 项目切换 / 打开。
//!
//! handler 只 emit `HostEffect`（宿主弹项目选择器、走打开流程），不直接碰 GPUI / shell。
//! 模块名 `workspace` 是内部代号，面向用户文案统一用「项目」。
//!
//! esc 走系统级 [`crate::commands::system::dismiss::DISMISS_TOP`]（scope=ProjectPicker）—— [`SHOW_PROJECTS_PICKER`] 推一条 dismiss token，esc 弹出后重新派发 [`DISMISS`]。

use crate::commands::args::MoveDeltaArgs;
use crate::commands::cid;
use crate::commands::emit;
use crate::commands::system::dismiss as dismiss_top;
use crate::{
    CommandArgs, CommandContext, CommandError, CommandOutcome, CommandRegistry, DismissScope,
    HostEffect, Invocation, KeyBindingContext, Keymap, NoArgs, ProjectEffect, SurfaceEffect,
};

pub const SHOW_PROJECTS_PICKER: &str = "workspace.show_projects_picker";
pub const OPEN_LOCAL_PROJECT: &str = "workspace.open_local_project";
pub const START_GIT_CLONE: &str = "workspace.start_git_clone";
pub const REMOVE_RECENT_PROJECT: &str = "workspace.remove_recent_project";
pub const MOVE_SELECTION: &str = "workspace.project_picker.move_selection";
pub const ACTIVATE: &str = "workspace.project_picker.activate";
pub const DISMISS: &str = "workspace.project_picker.dismiss";

/// 项目选择器拥有自己的键盘上下文：Up/Down/Enter 等非文本按键只在选择器聚焦时解释，不污染全局快捷键空间。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectPickerKeyContext;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectPickerBindingContext;

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

pub fn move_selection(delta: isize) -> Invocation {
    (cid(MOVE_SELECTION), MoveDeltaArgs { delta }.into())
}

pub fn activate() -> Invocation {
    (cid(ACTIVATE), CommandArgs::new())
}

pub fn dismiss() -> Invocation {
    (cid(DISMISS), CommandArgs::new())
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    let picker = KeyBindingContext::project_picker();

    registry
        .install(
            keymap,
            SHOW_PROJECTS_PICKER,
            "切换项目",
            Box::new(run_show_projects_picker),
        )
        .description("打开项目选择器，切换到最近项目或导入新项目。")
        .key("mod o");

    registry
        .install(
            keymap,
            OPEN_LOCAL_PROJECT,
            "从本地路径导入",
            emit(HostEffect::Project(ProjectEffect::OpenLocalProject)),
        )
        .key_in("mod l", picker);

    registry
        .install(
            keymap,
            START_GIT_CLONE,
            "从远程地址导入",
            emit(HostEffect::Project(ProjectEffect::StartGitClone)),
        )
        .key_in("mod g", picker);

    registry
        .install(
            keymap,
            REMOVE_RECENT_PROJECT,
            "移除最近项目",
            emit(HostEffect::Project(
                ProjectEffect::RemoveSelectedRecentProject,
            )),
        )
        .key_in("mod backspace", picker)
        .key_in("mod delete", picker);

    registry
        .install(
            keymap,
            MOVE_SELECTION,
            "移动项目选择器选中项",
            Box::new(run_move_selection),
        )
        .key_with_in("up", move_args(-1), picker)
        .key_with_in("down", move_args(1), picker);

    registry
        .install(
            keymap,
            ACTIVATE,
            "激活项目选择器选中项",
            emit(HostEffect::Project(ProjectEffect::ActivatePicker)),
        )
        .key_in("enter", picker)
        .key_in("return", picker);

    registry.install(keymap, DISMISS, "关闭项目选择器", Box::new(run_dismiss));

    dismiss_top::bind_esc(keymap, DismissScope::ProjectPicker, picker);
}

fn run_move_selection(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = MoveDeltaArgs::try_from(args)?;
    context
        .effects
        .push(HostEffect::Project(ProjectEffect::MovePickerSelection(
            args.delta,
        )));
    Ok(CommandOutcome::default())
}

/// 打开选择器：清空 [`DismissScope::ProjectPicker`] 栈防止重复 open 导致 token 累积，
/// 再 push 一个 [`DISMISS`] token —— esc 走 [`crate::commands::system::dismiss::DISMISS_TOP`] 弹这条 token 把 picker 收掉。
fn run_show_projects_picker(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.dismiss.clear(DismissScope::ProjectPicker);
    context
        .dismiss
        .push(DismissScope::ProjectPicker, "关闭项目选择器", dismiss());
    context
        .effects
        .push(HostEffect::Project(ProjectEffect::ShowPicker));
    Ok(CommandOutcome::default())
}

/// 关闭选择器：清掉本 scope 上残留 token（万一被 host 直接调走，绕过了 esc 路径），再 emit [`HostEffect::Surface(SurfaceEffect::Dismiss)`]。
/// esc 路径上栈顶已经被 [`crate::commands::system::dismiss::DISMISS_TOP`] 弹掉，这里 [`crate::DismissStacks::clear`] 是幂等 no-op。
fn run_dismiss(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.dismiss.clear(DismissScope::ProjectPicker);
    context
        .effects
        .push(HostEffect::Surface(SurfaceEffect::Dismiss));
    Ok(CommandOutcome::default())
}

fn move_args(delta: isize) -> CommandArgs {
    MoveDeltaArgs { delta }.into()
}
