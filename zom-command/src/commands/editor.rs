//! `editor.*` 命令目录。
//!
//! 一站式声明：命令 id 常量 + typed args（双向转换）+ typed builders +
//! handler + 默认键位。新增 / 修改命令只改本文件。
//!
//! 调用约定：
//! ```ignore
//! // 1. 注册（在组合根：通常 App::new()）
//! editor::install(&mut registry, &mut keymap)?;
//!
//! // 2. 调用（类型安全，不再手拼字符串）
//! let invocation = editor::insert_text("hi");
//! app.dispatch(invocation);
//! ```

use zom_engine::{ByteOffset, Motion, MovementDirection, MovementUnit, Selection, SelectionSet};

use crate::{
    CommandArgs, CommandContext, CommandError, CommandId, CommandOutcome, CommandRegistry,
    Invocation, Keymap, NoArgs, command_execution_failed, parse_optional_bool, reject_unknown_args,
    required_arg, set_active_view_selection,
};
use zom_workspace::BufferId;

// ==================================================
// 命令 id —— 单一真理源
// ==================================================

pub const INSERT_TEXT: &str = "editor.insert_text";
pub const REPLACE_SELECTION: &str = "editor.replace_selection";
pub const INSERT_NEWLINE: &str = "editor.insert_newline";
pub const INDENT: &str = "editor.indent";
pub const OUTDENT: &str = "editor.outdent";
pub const DELETE_BACKWARD: &str = "editor.delete_backward";
pub const DELETE_FORWARD: &str = "editor.delete_forward";
pub const SELECT_ALL: &str = "editor.select_all";
pub const UNDO: &str = "editor.undo";
pub const REDO: &str = "editor.redo";
pub const MOVE_SELECTION: &str = "editor.move_selection";
pub const IME_COMMIT: &str = "editor.ime_commit";
pub const IME_CANCEL: &str = "editor.ime_cancel";

// ==================================================
// Typed builders 工具
// ==================================================

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}

// ==================================================
// Typed args + 双向转换
// ==================================================

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InsertTextArgs {
    pub text: String,
}

impl From<InsertTextArgs> for CommandArgs {
    fn from(args: InsertTextArgs) -> Self {
        CommandArgs::new().with("text", args.text)
    }
}

impl TryFrom<CommandArgs> for InsertTextArgs {
    type Error = CommandError;
    fn try_from(args: CommandArgs) -> Result<Self, Self::Error> {
        reject_unknown_args(&args, &["text"])?;
        Ok(Self {
            text: required_arg(&args, "text")?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplaceSelectionArgs {
    pub text: String,
}

impl From<ReplaceSelectionArgs> for CommandArgs {
    fn from(args: ReplaceSelectionArgs) -> Self {
        CommandArgs::new().with("text", args.text)
    }
}

impl TryFrom<CommandArgs> for ReplaceSelectionArgs {
    type Error = CommandError;
    fn try_from(args: CommandArgs) -> Result<Self, Self::Error> {
        reject_unknown_args(&args, &["text"])?;
        Ok(Self {
            text: required_arg(&args, "text")?,
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImeCommitArgs {
    pub text: String,
}

impl From<ImeCommitArgs> for CommandArgs {
    fn from(args: ImeCommitArgs) -> Self {
        CommandArgs::new().with("text", args.text)
    }
}

impl TryFrom<CommandArgs> for ImeCommitArgs {
    type Error = CommandError;
    fn try_from(args: CommandArgs) -> Result<Self, Self::Error> {
        reject_unknown_args(&args, &["text"])?;
        Ok(Self {
            text: args.get("text").unwrap_or("").to_string(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MoveSelectionArgs {
    pub direction: MovementDirection,
    pub motion: Motion,
    pub extend: bool,
}

impl From<MoveSelectionArgs> for CommandArgs {
    fn from(args: MoveSelectionArgs) -> Self {
        let mut out = CommandArgs::new()
            .with("direction", direction_to_str(args.direction))
            .with("motion", motion_to_str(args.motion))
            .with("extend", if args.extend { "true" } else { "false" });
        if let Motion::PageStep { lines } = args.motion {
            out = out.with("lines", lines.to_string());
        }
        out
    }
}

impl TryFrom<CommandArgs> for MoveSelectionArgs {
    type Error = CommandError;
    fn try_from(args: CommandArgs) -> Result<Self, Self::Error> {
        reject_unknown_args(&args, &["direction", "motion", "extend", "lines"])?;
        let motion_kind = required_arg(&args, "motion")?;
        Ok(Self {
            direction: parse_direction(&required_arg(&args, "direction")?)?,
            motion: parse_motion(&motion_kind, &args)?,
            extend: parse_optional_bool(args.get("extend"))?,
        })
    }
}

// ==================================================
// Typed builders —— 调用方一律走这里，不再手拼字符串
// ==================================================

pub fn insert_text(text: impl Into<String>) -> Invocation {
    (
        cid(INSERT_TEXT),
        InsertTextArgs { text: text.into() }.into(),
    )
}

pub fn replace_selection(text: impl Into<String>) -> Invocation {
    (
        cid(REPLACE_SELECTION),
        ReplaceSelectionArgs { text: text.into() }.into(),
    )
}

pub fn insert_newline() -> Invocation {
    (cid(INSERT_NEWLINE), CommandArgs::new())
}

pub fn indent() -> Invocation {
    (cid(INDENT), CommandArgs::new())
}

pub fn outdent() -> Invocation {
    (cid(OUTDENT), CommandArgs::new())
}

pub fn delete_backward() -> Invocation {
    (cid(DELETE_BACKWARD), CommandArgs::new())
}

pub fn delete_forward() -> Invocation {
    (cid(DELETE_FORWARD), CommandArgs::new())
}

pub fn select_all() -> Invocation {
    (cid(SELECT_ALL), CommandArgs::new())
}

pub fn undo() -> Invocation {
    (cid(UNDO), CommandArgs::new())
}

pub fn redo() -> Invocation {
    (cid(REDO), CommandArgs::new())
}

pub fn move_selection(
    direction: MovementDirection,
    motion: impl Into<Motion>,
    extend: bool,
) -> Invocation {
    let args = MoveSelectionArgs {
        direction,
        motion: motion.into(),
        extend,
    };
    (cid(MOVE_SELECTION), args.into())
}

pub fn ime_commit(text: impl Into<String>) -> Invocation {
    (cid(IME_COMMIT), ImeCommitArgs { text: text.into() }.into())
}

pub fn ime_cancel() -> Invocation {
    (cid(IME_CANCEL), CommandArgs::new())
}

// ==================================================
// 注册与默认键位 —— 同处声明
// ==================================================

/// 一次性注册全部 `editor.*` 命令与默认键位。
///
/// 默认键位采用逻辑修饰键（`mod / alt / shift`），平台投影在 UI 层完成；
/// 见 `zom-desktop/src/shell/keymap_format.rs`。
pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    // 没有默认键位的命令（文本输入走 IME；命令面板 / AI 直接调用）只 install 不 .key。
    registry.install(keymap, INSERT_TEXT, "插入文本", Box::new(run_insert_text));
    registry.install(
        keymap,
        REPLACE_SELECTION,
        "替换选区",
        Box::new(run_replace_selection),
    );
    registry.install(
        keymap,
        IME_COMMIT,
        "提交输入法组合",
        Box::new(run_ime_commit),
    );

    registry
        .install(
            keymap,
            INSERT_NEWLINE,
            "插入换行",
            Box::new(run_insert_newline),
        )
        .key("enter")
        .key("return");
    registry
        .install(keymap, INDENT, "增加缩进", Box::new(run_indent))
        .key("tab");
    registry
        .install(keymap, OUTDENT, "减少缩进", Box::new(run_outdent))
        .key("shift-tab");
    registry
        .install(
            keymap,
            DELETE_BACKWARD,
            "向后删除",
            Box::new(run_delete_backward),
        )
        .key("backspace");
    registry
        .install(
            keymap,
            DELETE_FORWARD,
            "向前删除",
            Box::new(run_delete_forward),
        )
        .key("delete");
    registry
        .install(keymap, SELECT_ALL, "全选", Box::new(run_select_all))
        .key("mod-a");
    registry
        .install(keymap, UNDO, "撤销", Box::new(run_undo))
        .key("mod-z");
    registry
        .install(keymap, REDO, "重做", Box::new(run_redo))
        .key("mod-shift-z");
    registry
        .install(
            keymap,
            IME_CANCEL,
            "取消输入法组合",
            Box::new(run_ime_cancel),
        )
        .key("escape");

    // 光标 / 选区的全部 移动 / 扩展 变体共用一条命令，按预设 args 区分。
    use MovementDirection::*;
    use MovementUnit::*;
    registry
        .install(
            keymap,
            MOVE_SELECTION,
            "移动选区",
            Box::new(run_move_selection),
        )
        .key_with("up", move_args(Previous, Motion::LineStep, false))
        .key_with("down", move_args(Next, Motion::LineStep, false))
        // pageup / pagedown：lines 暂用固定 20 作 fallback；
        // TODO: view 层 ViewportState 加 visible_lines 字段后，由 handler 从当前 view 注入真实值。
        .key_with("pageup", move_args(Previous, Motion::PageStep { lines: 20 }, false))
        .key_with("pagedown", move_args(Next, Motion::PageStep { lines: 20 }, false))
        .key_with("left", move_args(Previous, Grapheme, false))
        .key_with("right", move_args(Next, Grapheme, false))
        .key_with("shift-up", move_args(Previous, Motion::LineStep, true))
        .key_with("shift-down", move_args(Next, Motion::LineStep, true))
        .key_with("shift-pageup", move_args(Previous, Motion::PageStep { lines: 20 }, true))
        .key_with("shift-pagedown", move_args(Next, Motion::PageStep { lines: 20 }, true))
        .key_with("shift-left", move_args(Previous, Grapheme, true))
        .key_with("shift-right", move_args(Next, Grapheme, true))
        .key_with("alt-left", move_args(Previous, Word, false))
        .key_with("alt-right", move_args(Next, Word, false))
        .key_with("alt-shift-left", move_args(Previous, Word, true))
        .key_with("alt-shift-right", move_args(Next, Word, true))
        .key_with("home", move_args(Previous, LineEdge, false))
        .key_with("end", move_args(Next, LineEdge, false))
        .key_with("shift-home", move_args(Previous, LineEdge, true))
        .key_with("shift-end", move_args(Next, LineEdge, true));
}

fn move_args(
    direction: MovementDirection,
    motion: impl Into<Motion>,
    extend: bool,
) -> CommandArgs {
    MoveSelectionArgs {
        direction,
        motion: motion.into(),
        extend,
    }
    .into()
}

// ==================================================
// 字符串 ↔ 枚举映射（args 序列化的唯一真理源）
// ==================================================

fn direction_to_str(direction: MovementDirection) -> &'static str {
    match direction {
        MovementDirection::Previous => "previous",
        MovementDirection::Next => "next",
    }
}

fn motion_to_str(motion: Motion) -> &'static str {
    match motion {
        Motion::ByUnit(MovementUnit::Grapheme) => "grapheme",
        Motion::ByUnit(MovementUnit::Word) => "word",
        Motion::ByUnit(MovementUnit::Identifier) => "identifier",
        Motion::ByUnit(MovementUnit::Subword) => "subword",
        Motion::ByUnit(MovementUnit::Symbol) => "symbol",
        Motion::ByUnit(MovementUnit::LineEdge) => "line-edge",
        Motion::LineStep => "line-step",
        // PageStep 的 lines 通过 args.lines 另行携带，motion 字段仍是扁平字符串。
        Motion::PageStep { .. } => "page-step",
    }
}

fn parse_direction(value: &str) -> Result<MovementDirection, CommandError> {
    match value {
        "previous" | "left" => Ok(MovementDirection::Previous),
        "next" | "right" => Ok(MovementDirection::Next),
        other => Err(CommandError::InvalidArgs(format!("未知移动方向：{other}"))),
    }
}

fn parse_motion(value: &str, args: &CommandArgs) -> Result<Motion, CommandError> {
    match value {
        "grapheme" | "character" | "char" => Ok(Motion::ByUnit(MovementUnit::Grapheme)),
        "word" => Ok(Motion::ByUnit(MovementUnit::Word)),
        "identifier" => Ok(Motion::ByUnit(MovementUnit::Identifier)),
        "subword" => Ok(Motion::ByUnit(MovementUnit::Subword)),
        "symbol" => Ok(Motion::ByUnit(MovementUnit::Symbol)),
        "line-edge" => Ok(Motion::ByUnit(MovementUnit::LineEdge)),
        "line-step" => Ok(Motion::LineStep),
        "page-step" => {
            let raw = args.get("lines").ok_or_else(|| {
                CommandError::InvalidArgs("page-step 需要 lines 参数".to_string())
            })?;
            let lines: u32 = raw
                .parse()
                .map_err(|_| CommandError::InvalidArgs(format!("无效 lines：{raw}")))?;
            if lines == 0 {
                return Err(CommandError::InvalidArgs("lines 必须 > 0".to_string()));
            }
            Ok(Motion::PageStep { lines })
        }
        other => Err(CommandError::InvalidArgs(format!("未知光标运动：{other}"))),
    }
}

// ==================================================
// Handlers
// ==================================================

fn run_insert_text(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = InsertTextArgs::try_from(args)?;
    let buffer_id = active_buffer_id(context)?;
    let selections = active_selection(context)?;
    let after = {
        let buffer = buffer_mut(context, buffer_id)?;
        buffer
            .insert_at_selections(selections, &args.text)
            .map_err(command_execution_failed)?;
        buffer.selection().clone()
    };
    set_active_view_selection(context, after)?;
    Ok(CommandOutcome::default())
}

fn run_replace_selection(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = ReplaceSelectionArgs::try_from(args)?;
    let buffer_id = active_buffer_id(context)?;
    let selections = active_selection(context)?;
    let after = {
        let buffer = buffer_mut(context, buffer_id)?;
        buffer
            .replace_selections(selections, &args.text)
            .map_err(command_execution_failed)?;
        buffer.selection().clone()
    };
    set_active_view_selection(context, after)?;
    Ok(CommandOutcome::default())
}

fn run_insert_newline(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let buffer_id = active_buffer_id(context)?;
    let selections = active_selection(context)?;
    let after = {
        let buffer = buffer_mut(context, buffer_id)?;
        buffer
            .insert_at_selections(selections, "\n")
            .map_err(command_execution_failed)?;
        buffer.selection().clone()
    };
    set_active_view_selection(context, after)?;
    Ok(CommandOutcome::default())
}

fn run_indent(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let buffer_id = active_buffer_id(context)?;
    let selections = active_selection(context)?;
    let after = {
        let buffer = buffer_mut(context, buffer_id)?;
        buffer
            .indent_at_selections(selections)
            .map_err(command_execution_failed)?;
        buffer.selection().clone()
    };
    set_active_view_selection(context, after)?;
    Ok(CommandOutcome::default())
}

fn run_outdent(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let buffer_id = active_buffer_id(context)?;
    let selections = active_selection(context)?;
    let after = {
        let buffer = buffer_mut(context, buffer_id)?;
        buffer
            .outdent_at_selections(selections)
            .map_err(command_execution_failed)?;
        buffer.selection().clone()
    };
    set_active_view_selection(context, after)?;
    Ok(CommandOutcome::default())
}

fn run_delete_backward(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let buffer_id = active_buffer_id(context)?;
    let selections = active_selection(context)?;
    let after = {
        let buffer = buffer_mut(context, buffer_id)?;
        buffer
            .delete_backward_at_selections(selections)
            .map_err(command_execution_failed)?;
        buffer.selection().clone()
    };
    set_active_view_selection(context, after)?;
    Ok(CommandOutcome::default())
}

fn run_delete_forward(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let buffer_id = active_buffer_id(context)?;
    let selections = active_selection(context)?;
    let after = {
        let buffer = buffer_mut(context, buffer_id)?;
        buffer
            .delete_forward_at_selections(selections)
            .map_err(command_execution_failed)?;
        buffer.selection().clone()
    };
    set_active_view_selection(context, after)?;
    Ok(CommandOutcome::default())
}

fn run_select_all(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let buffer_id = active_buffer_id(context)?;
    let selection = {
        let buffer = buffer_mut(context, buffer_id)?;
        let selection =
            SelectionSet::new(vec![Selection::new(ByteOffset::ZERO, buffer.len_bytes())]);
        buffer
            .set_selection(selection.clone())
            .map_err(command_execution_failed)?;
        selection
    };
    set_active_view_selection(context, selection)?;
    Ok(CommandOutcome::default())
}

fn run_undo(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let buffer_id = active_buffer_id(context)?;
    let after = {
        let buffer = buffer_mut(context, buffer_id)?;
        buffer.undo().map_err(command_execution_failed)?;
        buffer.selection().clone()
    };
    set_active_view_selection(context, after)?;
    Ok(CommandOutcome::default())
}

fn run_redo(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let buffer_id = active_buffer_id(context)?;
    let after = {
        let buffer = buffer_mut(context, buffer_id)?;
        buffer.redo().map_err(command_execution_failed)?;
        buffer.selection().clone()
    };
    set_active_view_selection(context, after)?;
    Ok(CommandOutcome::default())
}

fn run_move_selection(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = MoveSelectionArgs::try_from(args)?;
    let buffer_id = active_buffer_id(context)?;
    let selections = active_selection(context)?;
    let moved = {
        let buffer = buffer_mut(context, buffer_id)?;
        buffer
            .move_selections(selections, args.direction, args.motion, args.extend)
            .map_err(command_execution_failed)?
    };
    set_active_view_selection(context, moved)?;
    Ok(CommandOutcome::default())
}

fn run_ime_commit(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = ImeCommitArgs::try_from(args)?;
    let buffer_id = active_buffer_id(context)?;
    let after = {
        let buffer = buffer_mut(context, buffer_id)?;
        buffer
            .commit_composition(&args.text)
            .map_err(command_execution_failed)?;
        buffer.selection().clone()
    };
    set_active_view_selection(context, after)?;
    Ok(CommandOutcome::default())
}

fn run_ime_cancel(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let buffer_id = active_buffer_id(context)?;
    let after = {
        let buffer = buffer_mut(context, buffer_id)?;
        buffer
            .cancel_composition()
            .map_err(command_execution_failed)?;
        buffer.selection().clone()
    };
    set_active_view_selection(context, after)?;
    Ok(CommandOutcome::default())
}

// ==================================================
// Context helpers
// ==================================================

fn active_buffer_id(context: &CommandContext<'_>) -> Result<BufferId, CommandError> {
    context
        .views
        .active_view()
        .map(|view| view.buffer())
        .ok_or(CommandError::NoActiveView)
}

fn active_selection(context: &CommandContext<'_>) -> Result<SelectionSet, CommandError> {
    context
        .views
        .active_view()
        .map(|view| view.selection().clone())
        .ok_or(CommandError::NoActiveView)
}

fn buffer_mut<'a>(
    context: &'a mut CommandContext<'_>,
    buffer_id: BufferId,
) -> Result<&'a mut zom_engine::Buffer, CommandError> {
    Ok(context
        .workspace
        .buffer_mut(buffer_id)
        .ok_or(CommandError::BufferNotFound(buffer_id))?
        .buffer_mut())
}
