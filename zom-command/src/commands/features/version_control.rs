//! `version_control.*` 命令目录。

use crate::commands::cid;
use crate::{
    CommandArgs, CommandContext, CommandError, CommandOutcome, CommandRegistry, HostEffect,
    Invocation, KeyBindingContext, Keymap, NoArgs, PanelKind, VersionControlEffect,
    reject_unknown_args, required_arg,
};

/// 版本控制面板当前键盘模式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VersionControlKeyContext {
    pub mode: VersionControlKeyMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VersionControlKeyMode {
    /// 树形导航：上/下/回车/左/右/暂存切换。
    Navigate,
    /// 提交信息编辑：文本输入、换行、Esc 取消。
    CommitMessage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VersionControlBindingContext {
    pub mode: VersionControlKeyMode,
}

pub const MOVE_SELECTION: &str = "version_control.move_selection";
pub const TOGGLE: &str = "version_control.toggle";
pub const ACTIVATE: &str = "version_control.activate";
pub const COLLAPSE_OR_PARENT: &str = "version_control.collapse_or_parent";
pub const EXPAND_OR_INTO: &str = "version_control.expand_or_into";
pub const EDIT_COMMIT_MESSAGE: &str = "version_control.edit_commit_message";
pub const CANCEL_COMMIT_MESSAGE: &str = "version_control.cancel_commit_message";
pub const COMMIT: &str = "version_control.commit";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VcMoveArgs {
    pub delta: isize,
}

impl From<VcMoveArgs> for CommandArgs {
    fn from(args: VcMoveArgs) -> Self {
        CommandArgs::new().with("delta", args.delta.to_string())
    }
}

impl TryFrom<CommandArgs> for VcMoveArgs {
    type Error = CommandError;

    fn try_from(args: CommandArgs) -> Result<Self, Self::Error> {
        reject_unknown_args(&args, &["delta"])?;
        let raw = required_arg(&args, "delta")?;
        let delta = raw
            .parse()
            .map_err(|_| CommandError::InvalidArgs(format!("无效移动步长：{raw}")))?;
        Ok(Self { delta })
    }
}

pub fn move_selection(delta: isize) -> Invocation {
    (cid(MOVE_SELECTION), VcMoveArgs { delta }.into())
}

pub fn toggle() -> Invocation {
    (cid(TOGGLE), CommandArgs::new())
}

pub fn activate() -> Invocation {
    (cid(ACTIVATE), CommandArgs::new())
}

pub fn collapse_or_parent() -> Invocation {
    (cid(COLLAPSE_OR_PARENT), CommandArgs::new())
}

pub fn expand_or_into() -> Invocation {
    (cid(EXPAND_OR_INTO), CommandArgs::new())
}

pub fn edit_commit_message() -> Invocation {
    (cid(EDIT_COMMIT_MESSAGE), CommandArgs::new())
}

pub fn cancel_commit_message() -> Invocation {
    (cid(CANCEL_COMMIT_MESSAGE), CommandArgs::new())
}

pub fn commit() -> Invocation {
    (cid(COMMIT), CommandArgs::new())
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    super::register_panel_toggle(
        registry,
        keymap,
        PanelKind::VersionControl,
        "版本管理",
        "打开或关闭版本管理面板。",
        "mod shift g",
    );

    let vc_navigate = KeyBindingContext::version_control(VersionControlKeyMode::Navigate);
    let vc_commit_msg = KeyBindingContext::version_control(VersionControlKeyMode::CommitMessage);

    registry
        .install(
            keymap,
            MOVE_SELECTION,
            "移动版本管理选中项",
            Box::new(run_move_selection),
        )
        .key_with_in("up", move_args(-1), vc_navigate)
        .key_with_in("down", move_args(1), vc_navigate);

    registry
        .install(
            keymap,
            ACTIVATE,
            "激活版本管理条目（文件=打开，目录=折叠展开）",
            Box::new(run_activate),
        )
        .key_in("enter", vc_navigate);

    registry.install(
        keymap,
        TOGGLE,
        "折叠或展开版本管理目录",
        Box::new(run_toggle),
    );

    registry
        .install(
            keymap,
            COLLAPSE_OR_PARENT,
            "折叠目录或跳转到父目录",
            Box::new(run_collapse_or_parent),
        )
        .key_in("left", vc_navigate);

    registry
        .install(
            keymap,
            EXPAND_OR_INTO,
            "展开目录或进入子项",
            Box::new(run_expand_or_into),
        )
        .key_in("right", vc_navigate);

    // ── 提交信息编辑 ──

    registry
        .install(
            keymap,
            EDIT_COMMIT_MESSAGE,
            "编辑提交信息",
            Box::new(run_edit_commit_message),
        )
        .key_in("c", vc_navigate);

    registry
        .install(
            keymap,
            CANCEL_COMMIT_MESSAGE,
            "取消提交信息编辑",
            Box::new(run_cancel_commit_message),
        )
        .key_in("escape", vc_commit_msg);

    // COMMIT 命令只注册不绑键盘快捷键，由 Glyph 按钮触发。
    registry.install(keymap, COMMIT, "提交变更", Box::new(run_commit));
}

fn run_move_selection(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = VcMoveArgs::try_from(args)?;
    context.effects.push(HostEffect::VersionControl(
        VersionControlEffect::MoveSelection(args.delta),
    ));
    Ok(CommandOutcome::default())
}

fn run_activate(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context
        .effects
        .push(HostEffect::VersionControl(VersionControlEffect::Activate));
    Ok(CommandOutcome::default())
}

fn run_toggle(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context
        .effects
        .push(HostEffect::VersionControl(VersionControlEffect::Toggle));
    Ok(CommandOutcome::default())
}

fn run_collapse_or_parent(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.effects.push(HostEffect::VersionControl(
        VersionControlEffect::CollapseOrParent,
    ));
    Ok(CommandOutcome::default())
}

fn run_expand_or_into(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.effects.push(HostEffect::VersionControl(
        VersionControlEffect::ExpandOrInto,
    ));
    Ok(CommandOutcome::default())
}

fn run_edit_commit_message(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.effects.push(HostEffect::VersionControl(
        VersionControlEffect::EditCommitMessage,
    ));
    Ok(CommandOutcome::default())
}

fn run_cancel_commit_message(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.effects.push(HostEffect::VersionControl(
        VersionControlEffect::CancelCommitMessage,
    ));
    Ok(CommandOutcome::default())
}

fn run_commit(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context
        .effects
        .push(HostEffect::VersionControl(VersionControlEffect::Commit));
    Ok(CommandOutcome::default())
}

fn move_args(delta: isize) -> CommandArgs {
    VcMoveArgs { delta }.into()
}
