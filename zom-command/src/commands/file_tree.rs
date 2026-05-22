//! `file_tree.*` 命令目录。
//!
//! 文件树命令只描述“用户意图”，实际修改 `FileTreeModel` 由宿主解释
//! [`HostEffect`] 完成。这样 `zom-command` 负责命令与快捷键，仍不反向依赖
//! `zom-desktop` 的面板实现。

use zom_workspace::EntryKind;

use crate::{
    CommandArgs, CommandContext, CommandError, CommandId, CommandOutcome, CommandRegistry,
    HostEffect, Invocation, KeyBindingContext, Keymap, NoArgs, reject_unknown_args, required_arg,
};

pub const MOVE_SELECTION: &str = "file_tree.move_selection";
pub const COLLAPSE_OR_PARENT: &str = "file_tree.collapse_or_parent";
pub const EXPAND_OR_INTO: &str = "file_tree.expand_or_into";
pub const ACTIVATE: &str = "file_tree.activate";
pub const FOCUS_EDITOR: &str = "file_tree.focus_editor";
pub const BEGIN_NEW_ENTRY: &str = "file_tree.begin_new_entry";
pub const COMMIT_NEW_ENTRY: &str = "file_tree.commit_new_entry";
pub const CANCEL_NEW_ENTRY: &str = "file_tree.cancel_new_entry";
pub const REQUEST_DELETE: &str = "file_tree.request_delete";
pub const CONFIRM_DELETE: &str = "file_tree.confirm_delete";
pub const CANCEL_DELETE: &str = "file_tree.cancel_delete";

/// 文件树当前键盘模式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileTreeKeyContext {
    pub mode: FileTreeKeyMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileTreeKeyMode {
    Navigate,
    PendingName,
    /// 删除确认弹窗打开中：只响应确认 / 取消。
    PendingDelete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileTreeBindingContext {
    pub mode: FileTreeKeyMode,
}

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
            .map_err(|_| CommandError::InvalidArgs(format!("无效文件树移动步长：{raw}")))?;
        Ok(Self { delta })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeginNewEntryArgs {
    pub kind: EntryKind,
}

impl From<BeginNewEntryArgs> for CommandArgs {
    fn from(args: BeginNewEntryArgs) -> Self {
        CommandArgs::new().with("kind", entry_kind_to_str(args.kind))
    }
}

impl TryFrom<CommandArgs> for BeginNewEntryArgs {
    type Error = CommandError;

    fn try_from(args: CommandArgs) -> Result<Self, Self::Error> {
        reject_unknown_args(&args, &["kind"])?;
        Ok(Self {
            kind: parse_entry_kind(&required_arg(&args, "kind")?)?,
        })
    }
}

pub fn move_selection(delta: isize) -> Invocation {
    (cid(MOVE_SELECTION), MoveSelectionArgs { delta }.into())
}

pub fn collapse_or_parent() -> Invocation {
    (cid(COLLAPSE_OR_PARENT), CommandArgs::new())
}

pub fn expand_or_into() -> Invocation {
    (cid(EXPAND_OR_INTO), CommandArgs::new())
}

pub fn activate() -> Invocation {
    (cid(ACTIVATE), CommandArgs::new())
}

pub fn focus_editor() -> Invocation {
    (cid(FOCUS_EDITOR), CommandArgs::new())
}

pub fn begin_new_entry(kind: EntryKind) -> Invocation {
    (cid(BEGIN_NEW_ENTRY), BeginNewEntryArgs { kind }.into())
}

pub fn commit_new_entry() -> Invocation {
    (cid(COMMIT_NEW_ENTRY), CommandArgs::new())
}

pub fn cancel_new_entry() -> Invocation {
    (cid(CANCEL_NEW_ENTRY), CommandArgs::new())
}

pub fn request_delete() -> Invocation {
    (cid(REQUEST_DELETE), CommandArgs::new())
}

pub fn confirm_delete() -> Invocation {
    (cid(CONFIRM_DELETE), CommandArgs::new())
}

pub fn cancel_delete() -> Invocation {
    (cid(CANCEL_DELETE), CommandArgs::new())
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    let navigate = KeyBindingContext::file_tree(FileTreeKeyMode::Navigate);
    let pending_name = KeyBindingContext::file_tree(FileTreeKeyMode::PendingName);
    let pending_delete = KeyBindingContext::file_tree(FileTreeKeyMode::PendingDelete);

    registry
        .install(
            keymap,
            MOVE_SELECTION,
            "移动文件树选中项",
            Box::new(run_move_selection),
        )
        .key_with_in("up", move_args(-1), navigate)
        .key_with_in("down", move_args(1), navigate);

    registry
        .install(
            keymap,
            COLLAPSE_OR_PARENT,
            "折叠文件树条目或跳到父目录",
            Box::new(run_collapse_or_parent),
        )
        .key_in("left", navigate);

    registry
        .install(
            keymap,
            EXPAND_OR_INTO,
            "展开文件树条目或进入子项",
            Box::new(run_expand_or_into),
        )
        .key_in("right", navigate);

    registry
        .install(keymap, ACTIVATE, "激活文件树条目", Box::new(run_activate))
        .key_in("enter", navigate);

    registry
        .install(
            keymap,
            FOCUS_EDITOR,
            "文件树焦点回到编辑器",
            Box::new(run_focus_editor),
        )
        .key_in("escape", navigate);

    registry
        .install(
            keymap,
            BEGIN_NEW_ENTRY,
            "在文件树中新建条目",
            Box::new(run_begin_new_entry),
        )
        .key_with_in("mod-n", begin_new_entry_args(EntryKind::File), navigate)
        .key_with_in(
            "mod-shift-n",
            begin_new_entry_args(EntryKind::Directory),
            navigate,
        );

    registry
        .install(
            keymap,
            COMMIT_NEW_ENTRY,
            "提交文件树新建条目",
            Box::new(run_commit_new_entry),
        )
        .key_in("enter", pending_name);

    registry
        .install(
            keymap,
            CANCEL_NEW_ENTRY,
            "取消文件树新建条目",
            Box::new(run_cancel_new_entry),
        )
        .key_in("escape", pending_name);

    registry
        .install(
            keymap,
            REQUEST_DELETE,
            "删除文件树选中条目",
            Box::new(run_request_delete),
        )
        .key_in("mod-delete", navigate)
        .key_in("mod-backspace", navigate);

    registry
        .install(
            keymap,
            CONFIRM_DELETE,
            "确认删除文件树条目",
            Box::new(run_confirm_delete),
        )
        .key_in("enter", pending_delete);

    registry
        .install(
            keymap,
            CANCEL_DELETE,
            "取消删除文件树条目",
            Box::new(run_cancel_delete),
        )
        .key_in("escape", pending_delete);
}

fn run_move_selection(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = MoveSelectionArgs::try_from(args)?;
    context
        .effects
        .push(HostEffect::FileTreeMoveSelection(args.delta));
    Ok(CommandOutcome::default())
}

fn run_collapse_or_parent(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.effects.push(HostEffect::FileTreeCollapseOrParent);
    Ok(CommandOutcome::default())
}

fn run_expand_or_into(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.effects.push(HostEffect::FileTreeExpandOrInto);
    Ok(CommandOutcome::default())
}

fn run_activate(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.effects.push(HostEffect::FileTreeActivate);
    Ok(CommandOutcome::default())
}

fn run_focus_editor(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.effects.push(HostEffect::FileTreeFocusEditor);
    Ok(CommandOutcome::default())
}

fn run_begin_new_entry(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = BeginNewEntryArgs::try_from(args)?;
    context
        .effects
        .push(HostEffect::FileTreeBeginNewEntry(args.kind));
    Ok(CommandOutcome::default())
}

fn run_commit_new_entry(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.effects.push(HostEffect::FileTreeCommitNewEntry);
    Ok(CommandOutcome::default())
}

fn run_cancel_new_entry(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.effects.push(HostEffect::FileTreeCancelNewEntry);
    Ok(CommandOutcome::default())
}

fn run_request_delete(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.effects.push(HostEffect::FileTreeRequestDelete);
    Ok(CommandOutcome::default())
}

fn run_confirm_delete(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.effects.push(HostEffect::FileTreeConfirmDelete);
    Ok(CommandOutcome::default())
}

fn run_cancel_delete(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.effects.push(HostEffect::FileTreeCancelDelete);
    Ok(CommandOutcome::default())
}

fn move_args(delta: isize) -> CommandArgs {
    MoveSelectionArgs { delta }.into()
}

fn begin_new_entry_args(kind: EntryKind) -> CommandArgs {
    BeginNewEntryArgs { kind }.into()
}

fn entry_kind_to_str(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "file",
        EntryKind::Directory => "directory",
    }
}

fn parse_entry_kind(raw: &str) -> Result<EntryKind, CommandError> {
    match raw {
        "file" => Ok(EntryKind::File),
        "directory" => Ok(EntryKind::Directory),
        other => Err(CommandError::InvalidArgs(format!(
            "未知文件树条目类型：{other}"
        ))),
    }
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
