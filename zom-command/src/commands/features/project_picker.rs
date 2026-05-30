//! `workspace.*` 命令目录 —— 项目切换 / 打开。
//!
//! handler 只 emit `HostEffect`（宿主弹项目选择器、走打开流程），不直接碰
//! GPUI / shell。模块名 `workspace` 是内部代号，面向用户文案统一用「项目」。

use crate::{
    CommandArgs, CommandContext, CommandError, CommandHandler, CommandId, CommandOutcome,
    CommandRegistry, HostEffect, Invocation, KeyBindingContext, Keymap, NoArgs,
    reject_unknown_args, required_arg,
};

pub const SHOW_PROJECTS_PICKER: &str = "workspace.show_projects_picker";
pub const OPEN_LOCAL_PROJECT: &str = "workspace.open_local_project";
pub const START_GIT_CLONE: &str = "workspace.start_git_clone";
pub const REMOVE_RECENT_PROJECT: &str = "workspace.remove_recent_project";
pub const MOVE_SELECTION: &str = "workspace.project_picker.move_selection";
pub const ACTIVATE: &str = "workspace.project_picker.activate";

/// 项目选择器拥有自己的键盘上下文：Up/Down/Enter 等非文本按键只在选择器
/// 聚焦时解释，不污染全局快捷键空间。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectPickerKeyContext;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectPickerBindingContext;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MoveSelectionArgs {
    pub delta: isize,
}

impl From<MoveSelectionArgs> for CommandArgs {
    fn from(args: MoveSelectionArgs) -> Self {
        CommandArgs::new().with("delta", args.delta.to_string())
    }
}

impl TryFrom<CommandArgs> for MoveSelectionArgs {
    type Error = CommandError;

    fn try_from(args: CommandArgs) -> Result<Self, Self::Error> {
        reject_unknown_args(&args, &["delta"])?;
        let raw = required_arg(&args, "delta")?;
        let delta = raw
            .parse()
            .map_err(|_| CommandError::InvalidArgs(format!("无效项目选择器移动步长：{raw}")))?;
        Ok(Self { delta })
    }
}

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
    (cid(MOVE_SELECTION), MoveSelectionArgs { delta }.into())
}

pub fn activate() -> Invocation {
    (cid(ACTIVATE), CommandArgs::new())
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    let picker = KeyBindingContext::project_picker();

    registry
        .install(
            keymap,
            SHOW_PROJECTS_PICKER,
            "切换项目",
            emit(HostEffect::ShowProjectPicker),
        )
        .description("打开项目选择器，切换到最近项目或导入新项目。")
        .key("mod-o");

    registry
        .install(
            keymap,
            OPEN_LOCAL_PROJECT,
            "从本地路径导入",
            emit(HostEffect::OpenLocalProject),
        )
        .key_in("mod-l", picker);

    registry
        .install(
            keymap,
            START_GIT_CLONE,
            "从 Git 地址导入",
            emit(HostEffect::StartGitClone),
        )
        .key_in("mod-g", picker);

    registry
        .install(
            keymap,
            REMOVE_RECENT_PROJECT,
            "移除最近项目",
            emit(HostEffect::RemoveSelectedRecentProject),
        )
        .key_in("mod-backspace", picker)
        .key_in("mod-delete", picker);

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
            Box::new(run_activate),
        )
        .key_in("enter", picker)
        .key_in("return", picker);
}

/// 与 `window.rs::emit` 同形态；catalog 里"按一个键就推一个 effect"的样板。
fn emit(effect: HostEffect) -> CommandHandler {
    Box::new(move |ctx, args| {
        NoArgs::try_from(args)?;
        ctx.effects.push(effect.clone());
        Ok(CommandOutcome::default())
    })
}

fn run_move_selection(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = MoveSelectionArgs::try_from(args)?;
    context
        .effects
        .push(HostEffect::ProjectPickerMoveSelection(args.delta));
    Ok(CommandOutcome::default())
}

fn run_activate(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.effects.push(HostEffect::ProjectPickerActivate);
    Ok(CommandOutcome::default())
}

fn move_args(delta: isize) -> CommandArgs {
    MoveSelectionArgs { delta }.into()
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
