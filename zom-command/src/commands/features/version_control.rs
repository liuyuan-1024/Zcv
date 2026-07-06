//! `version_control.*` 命令目录。

use crate::commands::args::MoveDeltaArgs;
use crate::commands::cid;
use crate::commands::emit;
use crate::commands::system::dismiss as dismiss_top;
use crate::{
    CommandArgs, CommandContext, CommandError, CommandOutcome, CommandRegistry, DismissScope,
    GitEffect, HostEffect, Invocation, KeyContext, Keymap, NoArgs, PanelKind, VersionControlEffect,
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

pub const MOVE_SELECTION: &str = "version_control.move_selection";
pub const TOGGLE: &str = "version_control.toggle";
pub const ACTIVATE: &str = "version_control.activate";
pub const COLLAPSE_OR_PARENT: &str = "version_control.collapse_or_parent";
pub const EXPAND_OR_INTO: &str = "version_control.expand_or_into";
pub const EDIT_COMMIT_MESSAGE: &str = "version_control.edit_commit_message";
pub const CANCEL_COMMIT_MESSAGE: &str = "version_control.cancel_commit_message";
pub const COMMIT: &str = "version_control.commit";
pub const FETCH: &str = "version_control.fetch";
pub const PULL: &str = "version_control.pull";
pub const PUSH: &str = "version_control.push";

pub fn move_selection(delta: isize) -> Invocation {
    (cid(MOVE_SELECTION), MoveDeltaArgs { delta }.into())
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

pub fn fetch() -> Invocation {
    (cid(FETCH), CommandArgs::new())
}

pub fn pull() -> Invocation {
    (cid(PULL), CommandArgs::new())
}

pub fn push() -> Invocation {
    (cid(PUSH), CommandArgs::new())
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

    let vc_navigate = KeyContext::version_control(VersionControlKeyMode::Navigate);
    let vc_commit_msg = KeyContext::version_control(VersionControlKeyMode::CommitMessage);

    registry
        .install(
            keymap,
            MOVE_SELECTION,
            "移动版本管理选中项",
            Box::new(run_move_selection),
        )
        .description("在版本管理面板中上下移动选中项。")
        .key_with_in("up", move_args(-1), vc_navigate)
        .key_with_in("down", move_args(1), vc_navigate);

    registry
        .install(
            keymap,
            ACTIVATE,
            "激活版本管理条目",
            emit(HostEffect::VersionControl(VersionControlEffect::Activate)),
        )
        .description("打开文件或折叠/展开目录。")
        .key_in("enter", vc_navigate);

    registry
        .install(
            keymap,
            TOGGLE,
            "暂存切换",
            emit(HostEffect::VersionControl(VersionControlEffect::Toggle)),
        )
        .description("暂存或取消暂存当前选中文件。")
        .key_in("space", vc_navigate);

    registry
        .install(
            keymap,
            COLLAPSE_OR_PARENT,
            "折叠或跳到父目录",
            emit(HostEffect::VersionControl(
                VersionControlEffect::CollapseOrParent,
            )),
        )
        .description("折叠当前展开的目录，或跳转到父级。")
        .key_in("left", vc_navigate);

    registry
        .install(
            keymap,
            EXPAND_OR_INTO,
            "展开或进入子项",
            emit(HostEffect::VersionControl(
                VersionControlEffect::ExpandOrInto,
            )),
        )
        .description("展开目录或进入子文件。")
        .key_in("right", vc_navigate);

    // ── 提交信息编辑（需 dismiss 栈，不能用 emit）──

    registry
        .install(
            keymap,
            EDIT_COMMIT_MESSAGE,
            "编辑提交信息",
            Box::new(run_edit_commit_message),
        )
        .description("进入提交信息编辑模式。")
        .key_in("c", vc_navigate);

    registry
        .install(
            keymap,
            CANCEL_COMMIT_MESSAGE,
            "取消提交信息编辑",
            Box::new(run_cancel_commit_message),
        )
        .hide_from_shortcuts();

    // COMMIT 命令只注册不绑键盘快捷键，由 Glyph 按钮触发。
    registry
        .install(keymap, COMMIT, "提交变更", Box::new(run_commit))
        .hide_from_shortcuts();

    // FETCH / PULL / PUSH 只注册不绑键盘快捷键，由顶栏 Glyph 触发。
    registry
        .install(
            keymap,
            FETCH,
            "获取远程更新",
            emit(HostEffect::Git(GitEffect::Fetch)),
        )
        .hide_from_shortcuts();

    registry
        .install(
            keymap,
            PULL,
            "拉取远程提交",
            emit(HostEffect::Git(GitEffect::Pull)),
        )
        .hide_from_shortcuts();

    registry
        .install(
            keymap,
            PUSH,
            "推送本地提交",
            emit(HostEffect::Git(GitEffect::Push)),
        )
        .hide_from_shortcuts();

    // esc 走 dismiss 栈统一路由（与 file_tree、search、go_to_line 一致）
    dismiss_top::bind_esc(keymap, DismissScope::VersionControl, vc_commit_msg);
}

fn run_move_selection(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = MoveDeltaArgs::try_from(args)?;
    context.effects.push(HostEffect::VersionControl(
        VersionControlEffect::MoveSelection(args.delta),
    ));
    Ok(CommandOutcome::default())
}

fn run_edit_commit_message(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.dismiss.clear(DismissScope::VersionControl);
    context.dismiss.push(
        DismissScope::VersionControl,
        "取消提交信息编辑",
        cancel_commit_message(),
    );
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
    context.dismiss.clear(DismissScope::VersionControl);
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
    context.dismiss.clear(DismissScope::VersionControl);
    context
        .effects
        .push(HostEffect::VersionControl(VersionControlEffect::Commit));
    Ok(CommandOutcome::default())
}

fn move_args(delta: isize) -> CommandArgs {
    MoveDeltaArgs { delta }.into()
}
