//! `branch_picker.*` 命令目录。

use crate::commands::args::MoveDeltaArgs;
use crate::commands::cid;
use crate::commands::emit;
use crate::commands::system::dismiss as dismiss_top;
use crate::{
    BranchEffect, CommandArgs, CommandContext, CommandError, CommandOutcome, CommandRegistry,
    DismissScope, HostEffect, Invocation, KeyBindingContext, Keymap, NoArgs, SurfaceEffect,
};

pub const SHOW_PICKER: &str = "branch_picker.show_picker";
pub const DISMISS: &str = "branch_picker.dismiss";
pub const MOVE_SELECTION: &str = "branch_picker.move_selection";
pub const SWITCH: &str = "branch_picker.switch";
pub const DELETE: &str = "branch_picker.delete";

pub fn show_picker() -> Invocation {
    (cid(SHOW_PICKER), CommandArgs::new())
}

pub fn dismiss() -> Invocation {
    (cid(DISMISS), CommandArgs::new())
}

pub fn move_selection(delta: isize) -> Invocation {
    (cid(MOVE_SELECTION), MoveDeltaArgs { delta }.into())
}

pub fn switch() -> Invocation {
    (cid(SWITCH), CommandArgs::new())
}

pub fn delete() -> Invocation {
    (cid(DELETE), CommandArgs::new())
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    let ctx = KeyBindingContext::branch_picker();

    registry
        .install(keymap, SHOW_PICKER, "切换分支", Box::new(run_show_picker))
        .key("mod shift b");

    registry.install(keymap, DISMISS, "关闭分支选择器", Box::new(run_dismiss));
    dismiss_top::bind_esc(keymap, DismissScope::BranchPicker, ctx);

    registry
        .install(
            keymap,
            MOVE_SELECTION,
            "移动分支选择",
            Box::new(run_move_selection),
        )
        .key_with_in("up", move_args(-1), ctx)
        .key_with_in("down", move_args(1), ctx);

    registry
        .install(
            keymap,
            SWITCH,
            "确认切换分支",
            emit(HostEffect::Branch(BranchEffect::Switch)),
        )
        .key_in("enter", ctx);

    registry
        .install(
            keymap,
            DELETE,
            "删除选中分支",
            emit(HostEffect::Branch(BranchEffect::DeleteSelected)),
        )
        .key_in("mod backspace", ctx)
        .key_in("mod delete", ctx);
}

fn run_show_picker(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.dismiss.clear(DismissScope::BranchPicker);
    context
        .dismiss
        .push(DismissScope::BranchPicker, "关闭分支选择器", dismiss());
    context
        .effects
        .push(HostEffect::Branch(BranchEffect::ShowPicker));
    Ok(CommandOutcome::default())
}

fn run_dismiss(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.dismiss.clear(DismissScope::BranchPicker);
    context
        .effects
        .push(HostEffect::Surface(SurfaceEffect::Dismiss));
    Ok(CommandOutcome::default())
}

fn run_move_selection(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let delta = MoveDeltaArgs::try_from(args)?.delta;
    context
        .effects
        .push(HostEffect::Branch(BranchEffect::MoveSelection(delta)));
    Ok(CommandOutcome::default())
}

fn move_args(delta: isize) -> CommandArgs {
    MoveDeltaArgs { delta }.into()
}
