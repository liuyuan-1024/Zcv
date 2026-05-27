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

use std::collections::BTreeSet;

use zom_engine::{
    Buffer, ByteOffset, EngineError, Line, Motion, MovementDirection, MovementUnit, Selection,
    SelectionSet, TextRange,
};

use crate::{
    CommandArgs, CommandContext, CommandError, CommandId, CommandOutcome, CommandRegistry,
    Invocation, KeyBindingContext, Keymap, NoArgs, active_view_buffer_id, command_execution_failed,
    parse_optional_bool, reject_unknown_args, required_arg,
};
use zom_view::ViewId;

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
pub const MOVE_SELECTION: &str = "editor.move_selection";
pub const SELECT_ALL: &str = "editor.select_all";
pub const UNDO: &str = "editor.undo";
pub const REDO: &str = "editor.redo";
pub const IME_COMMIT: &str = "editor.ime_commit";
pub const IME_CANCEL: &str = "editor.ime_cancel";
pub const IME_CONFIRM: &str = "editor.ime_confirm";
pub const SELECT_TAB: &str = "editor.select_tab";
pub const CLOSE_TAB: &str = "editor.close_tab";
pub const SAVE: &str = "editor.save";
pub const COPY: &str = "editor.copy";
pub const CUT: &str = "editor.cut";
pub const PASTE: &str = "editor.paste";

/// 文本编辑器当前能力。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextEditKeyContext {
    pub accepts_newline: bool,
    pub composing: bool,
}

/// 文本编辑命令的键位适用条件。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextEditBindingContext {
    pub requires_newline: bool,
    pub composition: CompositionBinding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompositionBinding {
    Any,
    Active,
    Inactive,
}

impl CompositionBinding {
    /// 两个组合态约束能否被同一运行时态同时满足。`Active` 与 `Inactive`
    /// 互斥；含 `Any` 的组合都有交集。
    pub(crate) fn overlaps(self, other: Self) -> bool {
        !matches!(
            (self, other),
            (Self::Active, Self::Inactive) | (Self::Inactive, Self::Active)
        )
    }
}

pub(crate) fn text_edit_context_matches(
    binding: TextEditBindingContext,
    active: TextEditKeyContext,
) -> bool {
    if binding.requires_newline && !active.accepts_newline {
        return false;
    }
    match binding.composition {
        CompositionBinding::Any => true,
        CompositionBinding::Active => active.composing,
        CompositionBinding::Inactive => !active.composing,
    }
}

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

/// `editor.select_tab` 的目标标签（一个 tab ↔ 一个 View）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectTabTarget {
    /// 下一个标签；越过末尾回到第一个。
    Next,
    /// 上一个标签；越过开头回到最后一个。
    Previous,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectTabArgs {
    pub target: SelectTabTarget,
}

impl From<SelectTabArgs> for CommandArgs {
    fn from(args: SelectTabArgs) -> Self {
        let target = match args.target {
            SelectTabTarget::Next => "next".to_string(),
            SelectTabTarget::Previous => "previous".to_string(),
        };
        CommandArgs::new().with("target", target)
    }
}

impl TryFrom<CommandArgs> for SelectTabArgs {
    type Error = CommandError;
    fn try_from(args: CommandArgs) -> Result<Self, Self::Error> {
        reject_unknown_args(&args, &["target"])?;
        let raw = required_arg(&args, "target")?;
        let target = match raw.as_str() {
            "next" => SelectTabTarget::Next,
            "previous" => SelectTabTarget::Previous,
            other => {
                return Err(CommandError::InvalidArgs(format!("未知标签目标：{other}")));
            }
        };
        Ok(Self { target })
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

pub fn select_all() -> Invocation {
    (cid(SELECT_ALL), CommandArgs::new())
}

pub fn undo() -> Invocation {
    (cid(UNDO), CommandArgs::new())
}

pub fn redo() -> Invocation {
    (cid(REDO), CommandArgs::new())
}

pub fn ime_commit(text: impl Into<String>) -> Invocation {
    (cid(IME_COMMIT), ImeCommitArgs { text: text.into() }.into())
}

pub fn ime_cancel() -> Invocation {
    (cid(IME_CANCEL), CommandArgs::new())
}

pub fn ime_confirm() -> Invocation {
    (cid(IME_CONFIRM), CommandArgs::new())
}

pub fn select_tab(target: SelectTabTarget) -> Invocation {
    (cid(SELECT_TAB), SelectTabArgs { target }.into())
}

pub fn close_tab() -> Invocation {
    (cid(CLOSE_TAB), CommandArgs::new())
}

/// 用于命令面板 / 菜单等以编程方式触发保存。键盘绑 `mod-s` 走 keymap 直派发。
pub fn save() -> Invocation {
    (cid(SAVE), CommandArgs::new())
}

pub fn copy() -> Invocation {
    (cid(COPY), CommandArgs::new())
}

pub fn cut() -> Invocation {
    (cid(CUT), CommandArgs::new())
}

pub fn paste() -> Invocation {
    (cid(PASTE), CommandArgs::new())
}

// ==================================================
// 注册与默认键位 —— 同处声明
// ==================================================

/// 一次性注册全部 `editor.*` 命令与默认键位。
///
/// 默认键位采用逻辑修饰键（`mod / alt / shift`），平台投影在 UI 层完成；
/// 见 `zom-desktop/src/shell/keymap_format.rs`。
pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    let text_edit = KeyBindingContext::text_edit();
    let text_edit_multiline = KeyBindingContext::text_edit_multiline();
    let text_edit_composition = KeyBindingContext::text_edit_composition();

    // 没有默认键位的文本输入命令（文本输入走 IME；命令面板 / AI 直接调用）只 install 不 .key。
    registry.install(keymap, INSERT_TEXT, "插入文本", Box::new(run_insert_text));
    registry.install(
        keymap,
        REPLACE_SELECTION,
        "替换选区",
        Box::new(run_replace_selection),
    );

    registry
        .install(
            keymap,
            INSERT_NEWLINE,
            "插入换行",
            Box::new(run_insert_newline),
        )
        .key_in("enter", text_edit_multiline)
        .key_in("return", text_edit_multiline);
    registry
        .install(keymap, INDENT, "增加缩进", Box::new(run_indent))
        .key_in("tab", text_edit_multiline);
    registry
        .install(keymap, OUTDENT, "减少缩进", Box::new(run_outdent))
        .key_in("shift-tab", text_edit_multiline);
    registry
        .install(
            keymap,
            DELETE_BACKWARD,
            "向后删除",
            Box::new(run_delete_backward),
        )
        .key_in("backspace", text_edit);
    registry
        .install(
            keymap,
            DELETE_FORWARD,
            "向前删除",
            Box::new(run_delete_forward),
        )
        .key_in("delete", text_edit);
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
        .key_with_in(
            "up",
            move_args(Previous, Motion::LineStep, false),
            text_edit,
        )
        .key_with_in("down", move_args(Next, Motion::LineStep, false), text_edit)
        // pageup / pagedown：keymap 里写死 20 行作首帧兜底；handler 里若主编辑区
        // 已测得 visible_line_count（element prepaint 反算写回），按真实值覆盖。
        .key_with_in(
            "pageup",
            move_args(Previous, Motion::PageStep { lines: 20 }, false),
            text_edit,
        )
        .key_with_in(
            "pagedown",
            move_args(Next, Motion::PageStep { lines: 20 }, false),
            text_edit,
        )
        .key_with_in("left", move_args(Previous, Grapheme, false), text_edit)
        .key_with_in("right", move_args(Next, Grapheme, false), text_edit)
        .key_with_in(
            "shift-up",
            move_args(Previous, Motion::LineStep, true),
            text_edit,
        )
        .key_with_in(
            "shift-down",
            move_args(Next, Motion::LineStep, true),
            text_edit,
        )
        .key_with_in(
            "shift-pageup",
            move_args(Previous, Motion::PageStep { lines: 20 }, true),
            text_edit,
        )
        .key_with_in(
            "shift-pagedown",
            move_args(Next, Motion::PageStep { lines: 20 }, true),
            text_edit,
        )
        .key_with_in("shift-left", move_args(Previous, Grapheme, true), text_edit)
        .key_with_in("shift-right", move_args(Next, Grapheme, true), text_edit)
        .key_with_in("alt-left", move_args(Previous, Word, false), text_edit)
        .key_with_in("alt-right", move_args(Next, Word, false), text_edit)
        .key_with_in("alt-shift-left", move_args(Previous, Word, true), text_edit)
        .key_with_in("alt-shift-right", move_args(Next, Word, true), text_edit)
        .key_with_in("home", move_args(Previous, LineEdge, false), text_edit)
        .key_with_in("end", move_args(Next, LineEdge, false), text_edit)
        .key_with_in("shift-home", move_args(Previous, LineEdge, true), text_edit)
        .key_with_in("shift-end", move_args(Next, LineEdge, true), text_edit);

    registry
        .install(keymap, SELECT_ALL, "全选", Box::new(run_select_all))
        .description("选中当前编辑器中的全部文本。")
        .key_in("mod-a", text_edit);
    registry
        .install(keymap, UNDO, "撤销", Box::new(run_undo))
        .description("撤销上一次编辑。")
        .key_in("mod-z", text_edit);
    registry
        .install(keymap, REDO, "重做", Box::new(run_redo))
        .description("重做上一次被撤销的编辑。")
        .key_in("mod-shift-z", text_edit);
    registry.install(
        keymap,
        IME_COMMIT,
        "提交输入法组合",
        Box::new(run_ime_commit),
    );
    registry
        .install(
            keymap,
            IME_CANCEL,
            "取消输入法组合",
            Box::new(run_ime_cancel),
        )
        .key_in("escape", text_edit_composition);
    registry
        .install(
            keymap,
            IME_CONFIRM,
            "确认输入法组合",
            Box::new(run_ime_confirm),
        )
        .key_in("enter", text_edit_composition)
        .key_in("return", text_edit_composition);

    // 标签切换 / 关闭：键盘驱动，不接鼠标。
    // 下/上一个用 mod-l/h；mod-w 关当前。
    registry
        .install(keymap, SELECT_TAB, "切换标签", Box::new(run_select_tab))
        .description("在编辑器标签之间切换。")
        .key_with_in("mod-l", select_tab_args(SelectTabTarget::Next), text_edit)
        .key_with_in(
            "mod-h",
            select_tab_args(SelectTabTarget::Previous),
            text_edit,
        );

    registry
        .install(keymap, CLOSE_TAB, "关闭标签", Box::new(run_close_tab))
        .description("关闭当前编辑器标签。")
        .key_in("mod-w", text_edit);

    registry
        .install(keymap, SAVE, "保存", Box::new(run_save))
        .description("保存当前打开的文件。")
        .key_in("mod-s", text_edit);

    registry
        .install(keymap, COPY, "复制", Box::new(run_copy))
        .description("复制选区文本；无选区时复制当前行（含换行符）。")
        .key_in("mod-c", text_edit);
    registry
        .install(keymap, CUT, "剪切", Box::new(run_cut))
        .description("剪切选区文本；无选区时剪切当前行（含换行符）。")
        .key_in("mod-x", text_edit);
    registry
        .install(keymap, PASTE, "粘贴", Box::new(run_paste))
        .description("将剪贴板内容写入选区。")
        .key_in("mod-v", text_edit);
}

fn move_args(direction: MovementDirection, motion: impl Into<Motion>, extend: bool) -> CommandArgs {
    MoveSelectionArgs {
        direction,
        motion: motion.into(),
        extend,
    }
    .into()
}

fn select_tab_args(target: SelectTabTarget) -> CommandArgs {
    SelectTabArgs { target }.into()
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
    let target = context.edit_target()?;
    let selections = target.selection.clone();
    target
        .buffer
        .insert_at_selections(selections, &args.text)
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_replace_selection(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = ReplaceSelectionArgs::try_from(args)?;
    let target = context.edit_target()?;
    let selections = target.selection.clone();
    target
        .buffer
        .replace_selections(selections, &args.text)
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_insert_newline(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let target = context.edit_target()?;
    let selections = target.selection.clone();
    target
        .buffer
        .insert_at_selections(selections, "\n")
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_indent(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let target = context.edit_target()?;
    let selections = target.selection.clone();
    target
        .buffer
        .indent_at_selections(selections)
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_outdent(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let target = context.edit_target()?;
    let selections = target.selection.clone();
    target
        .buffer
        .outdent_at_selections(selections)
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_delete_backward(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let target = context.edit_target()?;
    let selections = target.selection.clone();
    target
        .buffer
        .delete_backward_at_selections(selections)
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_delete_forward(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let target = context.edit_target()?;
    let selections = target.selection.clone();
    target
        .buffer
        .delete_forward_at_selections(selections)
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_select_all(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let target = context.edit_target()?;
    let selection = SelectionSet::new(vec![Selection::new(
        ByteOffset::ZERO,
        target.buffer.len_bytes(),
    )]);
    target
        .buffer
        .set_selection(selection.clone())
        .map_err(command_execution_failed)?;
    *target.selection = selection;
    Ok(CommandOutcome::default())
}

fn run_undo(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let target = context.edit_target()?;
    target.buffer.undo().map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_redo(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let target = context.edit_target()?;
    target.buffer.redo().map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_move_selection(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let mut args = MoveSelectionArgs::try_from(args)?;
    // PageStep 步长按真实视口高度走：element 上一帧 prepaint 已把测得的visible_line_count 写回 ViewportState，从这里读。
    // focused_field 模式下作用于输入框（通常单行），主编辑区的视口高度对它无意义，保留 keymap 兜底。
    // visible_line_count == 0（首帧 / headless）也走兜底。
    if let Motion::PageStep { lines } = &mut args.motion
        && context.focused_field.is_none()
        && let Some(view) = context.views.active_view()
    {
        let measured = view.viewport().visible_line_count;
        if measured > 0 {
            *lines = u32::try_from(measured).unwrap_or(u32::MAX);
        }
    }
    let target = context.edit_target()?;
    let selections = target.selection.clone();
    let moved = target
        .buffer
        .move_selections(selections, args.direction, args.motion, args.extend)
        .map_err(command_execution_failed)?;
    *target.selection = moved;
    Ok(CommandOutcome::default())
}

fn run_ime_commit(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = ImeCommitArgs::try_from(args)?;
    let target = context.edit_target()?;
    target
        .buffer
        .commit_composition(&args.text)
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_ime_cancel(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let target = context.edit_target()?;
    target
        .buffer
        .cancel_composition()
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_ime_confirm(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let target = context.edit_target()?;
    let Some(preedit) = target
        .buffer
        .composition()
        .map(|state| state.preedit_text().to_string())
    else {
        return Ok(CommandOutcome::default());
    };
    target
        .buffer
        .commit_composition(&preedit)
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_select_tab(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = SelectTabArgs::try_from(args)?;
    // tab 顺序即 ViewSet 的视图顺序（= 打开顺序）。
    let ids: Vec<ViewId> = context.views.views().map(|(id, _)| id).collect();
    if ids.is_empty() {
        return Ok(CommandOutcome::default());
    }
    let current = context
        .views
        .active()
        .and_then(|active| ids.iter().position(|id| *id == active));
    let target = match args.target {
        SelectTabTarget::Next => match current {
            Some(index) => (index + 1) % ids.len(),
            None => 0,
        },
        SelectTabTarget::Previous => match current {
            Some(index) => (index + ids.len() - 1) % ids.len(),
            None => 0,
        },
    };
    // target 已对标签数取模，必落在范围内；get 仅作防御。
    if let Some(id) = ids.get(target) {
        context.views.set_active(*id);
        sync_active_buffer(context);
    }
    Ok(CommandOutcome::default())
}

fn run_close_tab(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let Some(active) = context.views.active() else {
        return Ok(CommandOutcome::default());
    };
    // 只关视图，不关 buffer：dirty 内容仍留在 workspace，不丢数据。
    // 孤立 buffer 的回收留待后续（确认弹窗 / 引用计数）。
    context.views.close_view(active);
    sync_active_buffer(context);
    Ok(CommandOutcome::default())
}

fn run_save(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let buffer_id = active_view_buffer_id(context)?;
    context
        .workspace
        .save_file(buffer_id)
        .map_err(|error| CommandError::ExecutionFailed(error.to_string()))?;
    Ok(CommandOutcome::default())
}

fn run_copy(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let target = context.edit_target()?;
    if let Some(text) =
        collect_clipboard_text(target.buffer, target.selection).map_err(command_execution_failed)?
    {
        context.clipboard.write(&text);
    }
    Ok(CommandOutcome::default())
}

fn run_cut(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    // 先取出剪贴板文本 + 删除材料；过程中只借 target，借完即释放，
    // 之后才碰 `context.clipboard` 写入，避免对 context 的两次可变借用相撞。
    enum CutPlan {
        DeleteSelections(SelectionSet),
        DeleteLineRanges(Vec<TextRange>),
    }
    let (text, plan) = {
        let target = context.edit_target()?;
        let any_non_empty = target.selection.as_slice().iter().any(|s| !s.is_caret());
        let Some(text) = collect_clipboard_text(target.buffer, target.selection)
            .map_err(command_execution_failed)?
        else {
            return Ok(CommandOutcome::default());
        };
        let plan = if any_non_empty {
            CutPlan::DeleteSelections(target.selection.clone())
        } else {
            CutPlan::DeleteLineRanges(
                collect_caret_line_ranges(target.buffer, target.selection)
                    .map_err(command_execution_failed)?,
            )
        };
        (text, plan)
    };

    context.clipboard.write(&text);

    let target = context.edit_target()?;
    match plan {
        CutPlan::DeleteSelections(selections) => {
            target
                .buffer
                .delete_selection_ranges(selections)
                .map_err(command_execution_failed)?;
        }
        CutPlan::DeleteLineRanges(line_ranges) => {
            if !line_ranges.is_empty() {
                let line_selections = SelectionSet::from_ranges(line_ranges);
                target
                    .buffer
                    .delete_selection_ranges(line_selections)
                    .map_err(command_execution_failed)?;
            }
        }
    }
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_paste(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let Some(text) = context.clipboard.read() else {
        return Ok(CommandOutcome::default());
    };
    if text.is_empty() {
        return Ok(CommandOutcome::default());
    }
    let target = context.edit_target()?;
    let selections = target.selection.clone();
    target
        .buffer
        .replace_selections(selections, &text)
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

/// 收集要写入剪贴板的字符串。
///
/// 二选一规则：
/// - 任意非空 selection 存在 → 只取非空段，`\n` 分隔；
/// - 全部 caret → 每个 caret 取所在行（含 `\n`），按 [`Line`] 去重并按行号顺序，
///   直接拼接（行文本已自带 `\n`，末行无 `\n` 时不补）。
///
/// 没有可复制内容（全 caret 但 buffer 空 / 没有非空选区可取）返回 `None`。
fn collect_clipboard_text(
    buffer: &Buffer,
    selections: &SelectionSet,
) -> Result<Option<String>, EngineError> {
    let any_non_empty = selections.as_slice().iter().any(|s| !s.is_caret());
    if any_non_empty {
        let mut pieces: Vec<String> = Vec::new();
        for sel in selections.as_slice() {
            if sel.is_caret() {
                continue;
            }
            pieces.push(buffer.slice_text(sel.range())?.as_str().to_string());
        }
        if pieces.is_empty() {
            return Ok(None);
        }
        Ok(Some(pieces.join("\n")))
    } else {
        let lines = collect_caret_lines(buffer, selections)?;
        let mut out = String::new();
        for line in lines {
            out.push_str(buffer.slice_line(line)?.as_str());
        }
        if out.is_empty() {
            return Ok(None);
        }
        Ok(Some(out))
    }
}

/// 全 caret 模式下：把每个 caret 解析到所在行号，按 `Line` 去重并升序排列。
fn collect_caret_lines(
    buffer: &Buffer,
    selections: &SelectionSet,
) -> Result<Vec<Line>, EngineError> {
    let mut set: BTreeSet<Line> = BTreeSet::new();
    for sel in selections.as_slice() {
        if !sel.is_caret() {
            continue;
        }
        let pos = buffer.byte_to_position(sel.start())?;
        set.insert(pos.line());
    }
    Ok(set.into_iter().collect())
}

/// 全 caret 模式下：行号 → 整行 byte 范围（含 `\n`），供 cut 路径删除。
fn collect_caret_line_ranges(
    buffer: &Buffer,
    selections: &SelectionSet,
) -> Result<Vec<TextRange>, EngineError> {
    let lines = collect_caret_lines(buffer, selections)?;
    let mut ranges = Vec::with_capacity(lines.len());
    for line in lines {
        ranges.push(buffer.slice_line(line)?.range());
    }
    Ok(ranges)
}

/// 把 workspace 的活动 buffer 同步到当前活动视图——让文件树「活动文件」
/// 高亮跟随标签切换 / 关闭。无活动视图时（标签全关）保持原值不动。
fn sync_active_buffer(context: &mut CommandContext<'_>) {
    if let Some(buffer_id) = context.views.active_view().map(|view| view.buffer()) {
        let _ = context.workspace.set_active_buffer(buffer_id);
    }
}
