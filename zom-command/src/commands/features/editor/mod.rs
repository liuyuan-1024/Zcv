//! `editor.*` 命令目录。
//!
//! 一站式声明：命令 id 常量 + typed args（双向转换）+ typed builders + handler + 默认键位。
//!
//! 调用约定：
//! ```ignore
//! // 1. 注册（在组合根：通常 App::new()）
//! editor::install(&mut registry, &mut keymap)?;
//!
//! // 2. 调用（类型安全，不再手拼字符串）
//! let invocation = editor::insert_text("hi");
//! app.dispatch_command(invocation);
//! ```

use crate::commands::cid;
use std::collections::BTreeSet;

use crate::commands::system::dismiss as dismiss_top;
use crate::{
    BubbleRequest, CommandArgs, CommandContext, CommandError, CommandOutcome, CommandRegistry,
    DismissScope, HostEffect, Invocation, KeyBindingContext, Keymap, NoArgs, active_view_buffer_id,
    command_execution_failed, parse_optional_bool, reject_unknown_args, required_arg,
};
use zom_engine::{
    Buffer, ByteOffset, CompositionSelection, EngineError, Line, Motion, MovementDirection,
    MovementUnit, Selection, SelectionSet, TextRange, TransactionMetadata, TransactionSource,
    Utf16Offset,
};
use zom_workspace::view::ViewId;

mod visual_movement;
use visual_movement::move_target_selection;

// ==================================================
// 命令 id —— 单一真理源
// ==================================================

pub const INSERT_TEXT: &str = "editor.insert_text";
pub const REPLACE_SELECTION: &str = "editor.replace_selection";
pub const INSERT_NEWLINE: &str = "editor.insert_newline";
pub const INDENT: &str = "editor.indent";
pub const OUTDENT: &str = "editor.outdent";
pub const DELETE: &str = "editor.delete";
pub const MOVE_SELECTION: &str = "editor.move_selection";
pub const SELECT_ALL: &str = "editor.select_all";
/// 把每个选区塌成 caret（head 位置不变）。多 caret 不合并，只去 extent。
pub const CLEAR_SELECTION: &str = "editor.clear_selection";
pub const UNDO: &str = "editor.undo";
pub const REDO: &str = "editor.redo";
pub const IME_UPDATE: &str = "editor.ime_update";
pub const IME_COMMIT: &str = "editor.ime_commit";
pub const IME_CANCEL: &str = "editor.ime_cancel";
pub const IME_CONFIRM: &str = "editor.ime_confirm";
pub const SELECT_TAB: &str = "editor.select_tab";
pub const CLOSE_TAB: &str = "editor.close_tab";
pub const SAVE: &str = "editor.save";
pub const COPY: &str = "editor.copy";
pub const CUT: &str = "editor.cut";
pub const PASTE: &str = "editor.paste";
pub const TOGGLE_SOFT_WRAP: &str = "editor.toggle_soft_wrap";
pub const OPEN_PREVIEW: &str = "editor.preview.open";
pub const CHANGE_LANGUAGE: &str = "editor.change_language";

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
    /// 两个组合态约束能否被同一运行时态同时满足。
    /// `Active` 与 `Inactive` 互斥；含 `Any` 的组合都有交集。
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
    pub replacement_range_utf16: Option<ImeUtf16RangeArgs>,
    pub text: String,
}

impl From<ImeCommitArgs> for CommandArgs {
    fn from(args: ImeCommitArgs) -> Self {
        let mut out = CommandArgs::new().with("text", args.text);
        if let Some(range) = args.replacement_range_utf16 {
            out = range.write_to_args(out, "replacement");
        }
        out
    }
}

impl TryFrom<CommandArgs> for ImeCommitArgs {
    type Error = CommandError;
    fn try_from(args: CommandArgs) -> Result<Self, Self::Error> {
        reject_unknown_args(&args, &["text", "replacement_start", "replacement_end"])?;
        Ok(Self {
            replacement_range_utf16: ImeUtf16RangeArgs::parse_optional(&args, "replacement")?,
            text: args.get("text").unwrap_or("").to_string(),
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ImeUpdateArgs {
    pub replacement_range_utf16: Option<ImeUtf16RangeArgs>,
    pub text: String,
    pub selected_range_utf16: Option<ImeUtf16RangeArgs>,
}

impl From<ImeUpdateArgs> for CommandArgs {
    fn from(args: ImeUpdateArgs) -> Self {
        let mut out = CommandArgs::new().with("text", args.text);
        if let Some(range) = args.replacement_range_utf16 {
            out = range.write_to_args(out, "replacement");
        }
        if let Some(range) = args.selected_range_utf16 {
            out = range.write_to_args(out, "selected");
        }
        out
    }
}

impl TryFrom<CommandArgs> for ImeUpdateArgs {
    type Error = CommandError;
    fn try_from(args: CommandArgs) -> Result<Self, Self::Error> {
        reject_unknown_args(
            &args,
            &[
                "text",
                "replacement_start",
                "replacement_end",
                "selected_start",
                "selected_end",
            ],
        )?;
        Ok(Self {
            replacement_range_utf16: ImeUtf16RangeArgs::parse_optional(&args, "replacement")?,
            text: args.get("text").unwrap_or("").to_string(),
            selected_range_utf16: ImeUtf16RangeArgs::parse_optional(&args, "selected")?,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ImeUtf16RangeArgs {
    pub start: usize,
    pub end: usize,
}

impl ImeUtf16RangeArgs {
    pub fn new(start: usize, end: usize) -> Result<Self, CommandError> {
        if start > end {
            return Err(CommandError::InvalidArgs(
                "IME UTF-16 range start 大于 end".into(),
            ));
        }
        Ok(Self { start, end })
    }

    fn parse_optional(args: &CommandArgs, prefix: &str) -> Result<Option<Self>, CommandError> {
        let start_key = format!("{prefix}_start");
        let end_key = format!("{prefix}_end");
        match (args.get(&start_key), args.get(&end_key)) {
            (None, None) => Ok(None),
            (Some(start), Some(end)) => Ok(Some(Self::new(
                parse_usize_arg(start, &start_key)?,
                parse_usize_arg(end, &end_key)?,
            )?)),
            _ => Err(CommandError::InvalidArgs(format!(
                "IME {prefix} range 需要同时提供 start/end"
            ))),
        }
    }

    fn write_to_args(self, args: CommandArgs, prefix: &str) -> CommandArgs {
        args.with(format!("{prefix}_start"), self.start.to_string())
            .with(format!("{prefix}_end"), self.end.to_string())
    }
}

/// `editor.delete` 的参数集——与 [`MoveCaretArgs`] 同构（一条命令 + args 区分）。
///
/// 三种语义全在一组 args 里：
/// - `direction = Some(dir) + unit`：caret 沿 `dir` 删一个 `unit`；非空 selection 整段删。
/// - `direction = None`：caret 不动（no-op），仅删非空 selection。`unit` 此时无效，
///   args 里不允许出现 `motion`，否则报 `InvalidArgs`，避免「我提供了 unit 但被忽略」的隐性歧义。
///
/// PageStep / LineStep 对删除没有惯例语义，[`parse_unit`] 不接受。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeleteArgs {
    pub direction: Option<MovementDirection>,
    pub unit: MovementUnit,
}

impl From<DeleteArgs> for CommandArgs {
    fn from(args: DeleteArgs) -> Self {
        let mut out = CommandArgs::new();
        if let Some(dir) = args.direction {
            out = out.with("direction", direction_to_str(dir));
            // Grapheme 是默认值，省略 motion 让 keymap 文件最短。
            if args.unit != MovementUnit::Grapheme {
                out = out.with("motion", unit_to_str(args.unit));
            }
        }
        // direction = None 时 unit 无意义，不序列化。
        out
    }
}

impl TryFrom<CommandArgs> for DeleteArgs {
    type Error = CommandError;
    fn try_from(args: CommandArgs) -> Result<Self, Self::Error> {
        reject_unknown_args(&args, &["direction", "motion"])?;
        let direction = match args.get("direction") {
            None | Some("") => None,
            Some(value) => Some(parse_direction(value)?),
        };
        let unit = match args.get("motion") {
            None | Some("") => MovementUnit::Grapheme,
            Some(value) => {
                if direction.is_none() {
                    return Err(CommandError::InvalidArgs(
                        "motion 仅在提供 direction 时有效（无 direction 表示只删非空选区）"
                            .to_string(),
                    ));
                }
                parse_unit(value)?
            }
        };
        Ok(Self { direction, unit })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MoveCaretArgs {
    pub direction: MovementDirection,
    pub motion: Motion,
    pub extend: bool,
}

impl From<MoveCaretArgs> for CommandArgs {
    fn from(args: MoveCaretArgs) -> Self {
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

impl TryFrom<CommandArgs> for MoveCaretArgs {
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

/// 删除的唯一入口。三种语义全靠 [`DeleteArgs`] 区分：
///
/// - `DeleteArgs { direction: Some(prev), unit: Grapheme }`：backspace
/// - `DeleteArgs { direction: Some(next), unit: Word }`：alt delete
/// - `DeleteArgs { direction: None, .. }`：caret 不动，只删非空 selection
pub fn delete(args: DeleteArgs) -> Invocation {
    (cid(DELETE), args.into())
}

pub fn move_selection(
    direction: MovementDirection,
    motion: impl Into<Motion>,
    extend: bool,
) -> Invocation {
    let args = MoveCaretArgs {
        direction,
        motion: motion.into(),
        extend,
    };
    (cid(MOVE_SELECTION), args.into())
}

pub fn select_all() -> Invocation {
    (cid(SELECT_ALL), CommandArgs::new())
}

pub fn clear_selection() -> Invocation {
    (cid(CLEAR_SELECTION), CommandArgs::new())
}

pub fn undo() -> Invocation {
    (cid(UNDO), CommandArgs::new())
}

pub fn redo() -> Invocation {
    (cid(REDO), CommandArgs::new())
}

pub fn ime_update(
    replacement_range_utf16: Option<ImeUtf16RangeArgs>,
    text: impl Into<String>,
    selected_range_utf16: Option<ImeUtf16RangeArgs>,
) -> Invocation {
    (
        cid(IME_UPDATE),
        ImeUpdateArgs {
            replacement_range_utf16,
            text: text.into(),
            selected_range_utf16,
        }
        .into(),
    )
}

pub fn ime_commit(
    replacement_range_utf16: Option<ImeUtf16RangeArgs>,
    text: impl Into<String>,
) -> Invocation {
    (
        cid(IME_COMMIT),
        ImeCommitArgs {
            replacement_range_utf16,
            text: text.into(),
        }
        .into(),
    )
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

/// 关闭当前活动标签（键盘快捷键 / 程序化调用路径）。
/// handler 从 `context.active_view_id` 取目标 view。
pub fn close_active_tab() -> Invocation {
    (cid(CLOSE_TAB), CommandArgs::new())
}

/// 关闭指定 view（点击标签关闭 glyph 路径）。
/// args 包含序列化的 view_id，handler 反序列化时走 [`ViewId::from_u64`]。
pub fn close_tab_by_id(target: ViewId) -> Invocation {
    let args = CommandArgs::new().with("view_id", target.as_u64().to_string());
    (cid(CLOSE_TAB), args)
}

/// 用于命令面板 / 菜单等以编程方式触发保存。
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

pub fn toggle_soft_wrap() -> Invocation {
    (cid(TOGGLE_SOFT_WRAP), CommandArgs::new())
}

pub fn open_preview() -> Invocation {
    (cid(OPEN_PREVIEW), CommandArgs::new())
}

pub fn change_language() -> Invocation {
    (cid(CHANGE_LANGUAGE), CommandArgs::new())
}

fn run_open_preview(
    ctx: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let buffer_id = active_view_buffer_id(ctx)?;
    ctx.effects.push(HostEffect::EditorOpenPreview(buffer_id));
    Ok(CommandOutcome::default())
}

fn run_change_language(
    _ctx: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    _ctx.effects
        .push(HostEffect::ShowBubble(BubbleRequest::info(
            "切换语言功能尚未实现",
        )));
    Ok(CommandOutcome::default())
}

// ==================================================
// 注册与默认键位 —— 同处声明
// ==================================================

/// 一次性注册全部 `editor.*` 命令与默认键位。
///
/// 默认键位采用逻辑修饰键（`mod / alt / shift`），平台投影在 UI 层完成；
/// 见 `zom-desktop/src/shell/keymap_format.rs`。
const PAGE_STEP_LINES: u32 = 1;

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
        .key_in("shift tab", text_edit_multiline);

    // 删除的所有方向 / 单位变体共用一条命令，按预设 args 区分——与 MOVE_SELECTION 同形。
    registry
        .install(keymap, DELETE, "删除", Box::new(run_delete))
        .key_with_in(
            "backspace",
            delete_args(MovementDirection::Previous, MovementUnit::Grapheme),
            text_edit,
        )
        .key_with_in(
            "delete",
            delete_args(MovementDirection::Next, MovementUnit::Grapheme),
            text_edit,
        )
        // alt backspace / alt delete 与 alt left / alt right（按词移动）对称：按词删除。
        .key_with_in(
            "alt backspace",
            delete_args(MovementDirection::Previous, MovementUnit::Word),
            text_edit,
        )
        .key_with_in(
            "alt delete",
            delete_args(MovementDirection::Next, MovementUnit::Word),
            text_edit,
        );

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
        .key_with_in("left", move_args(Previous, Grapheme, false), text_edit)
        .key_with_in("right", move_args(Next, Grapheme, false), text_edit)
        // pageup / pagedown：keymap 里使用 PAGE_STEP_LINES 作为首帧兜底；
        // handler 里若主编辑区已测得 visible_visual_rows（element prepaint 反算写回），按真实值覆盖。
        .key_with_in(
            "pageup",
            move_args(
                Previous,
                Motion::PageStep {
                    lines: PAGE_STEP_LINES,
                },
                false,
            ),
            text_edit,
        )
        .key_with_in(
            "pagedown",
            move_args(
                Next,
                Motion::PageStep {
                    lines: PAGE_STEP_LINES,
                },
                false,
            ),
            text_edit,
        )
        .key_with_in(
            "shift up",
            move_args(Previous, Motion::LineStep, true),
            text_edit,
        )
        .key_with_in(
            "shift down",
            move_args(Next, Motion::LineStep, true),
            text_edit,
        )
        .key_with_in(
            "shift pageup",
            move_args(
                Previous,
                Motion::PageStep {
                    lines: PAGE_STEP_LINES,
                },
                true,
            ),
            text_edit,
        )
        .key_with_in(
            "shift pagedown",
            move_args(
                Next,
                Motion::PageStep {
                    lines: PAGE_STEP_LINES,
                },
                true,
            ),
            text_edit,
        )
        .key_with_in("shift left", move_args(Previous, Grapheme, true), text_edit)
        .key_with_in("shift right", move_args(Next, Grapheme, true), text_edit)
        .key_with_in("alt left", move_args(Previous, Word, false), text_edit)
        .key_with_in("alt right", move_args(Next, Word, false), text_edit)
        .key_with_in("alt shift left", move_args(Previous, Word, true), text_edit)
        .key_with_in("alt shift right", move_args(Next, Word, true), text_edit)
        .key_with_in("home", move_args(Previous, LineEdge, false), text_edit)
        .key_with_in("end", move_args(Next, LineEdge, false), text_edit)
        .key_with_in("shift home", move_args(Previous, LineEdge, true), text_edit)
        .key_with_in("shift end", move_args(Next, LineEdge, true), text_edit);

    registry
        .install(keymap, SELECT_ALL, "全选", Box::new(run_select_all))
        .description("选中当前编辑器中的全部文本。")
        .key_in("mod a", text_edit);

    registry.install(
        keymap,
        CLEAR_SELECTION,
        "取消选区",
        Box::new(run_clear_selection),
    );
    registry
        .install(keymap, UNDO, "撤销", Box::new(run_undo))
        .description("撤销上一次编辑。")
        .key_in("mod z", text_edit);
    registry
        .install(keymap, REDO, "重做", Box::new(run_redo))
        .description("重做上一次被撤销的编辑。")
        .key_in("mod shift z", text_edit);
    registry
        .install(
            keymap,
            IME_UPDATE,
            "更新输入法组合",
            Box::new(run_ime_update),
        )
        .hide_from_shortcuts();
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

    // 非 composition 态的 esc 路由到 system.dismiss_top(TextEdit)：
    // host 在选区扩展时往这条栈上 push 一条 [`CLEAR_SELECTION`] token，esc 弹出后塌掉选区。
    // composition Active 仍由上面的 IME_CANCEL 静态接管（两个 composition 维度互斥不冲突）。
    dismiss_top::bind_esc(keymap, DismissScope::TextEdit, text_edit);

    registry
        .install(keymap, SELECT_TAB, "切换标签", Box::new(run_select_tab))
        .description("在编辑器标签之间切换。")
        .key_with_in("mod l", select_tab_args(SelectTabTarget::Next), text_edit)
        .key_with_in(
            "mod h",
            select_tab_args(SelectTabTarget::Previous),
            text_edit,
        );
    registry
        .install(keymap, CLOSE_TAB, "关闭标签", Box::new(run_close_tab))
        .description("关闭当前编辑器标签。")
        .key_in("mod w", text_edit);

    registry
        .install(keymap, SAVE, "保存", Box::new(run_save))
        .description("保存当前打开的文件。")
        .key_in("mod s", text_edit);

    registry
        .install(keymap, COPY, "复制", Box::new(run_copy))
        .description("复制选区文本；无选区时复制当前行（含换行符）。")
        .key_in("mod c", text_edit);
    registry
        .install(keymap, CUT, "剪切", Box::new(run_cut))
        .description("剪切选区文本；无选区时剪切当前行（含换行符）。")
        .key_in("mod x", text_edit);
    registry
        .install(keymap, PASTE, "粘贴", Box::new(run_paste))
        .description("将剪贴板内容写入选区。")
        .key_in("mod v", text_edit);

    registry.install(
        keymap,
        TOGGLE_SOFT_WRAP,
        "切换软换行",
        Box::new(run_toggle_soft_wrap),
    );

    registry
        .install(keymap, OPEN_PREVIEW, "预览", Box::new(run_open_preview))
        .description("打开或跳转到当前文件的预览标签页。")
        .key_in("alt p", text_edit);

    super::go_to_line::install(registry, keymap);

    registry.install(
        keymap,
        CHANGE_LANGUAGE,
        "切换语言",
        Box::new(run_change_language),
    );
}

fn move_args(direction: MovementDirection, motion: impl Into<Motion>, extend: bool) -> CommandArgs {
    MoveCaretArgs {
        direction,
        motion: motion.into(),
        extend,
    }
    .into()
}

fn delete_args(direction: MovementDirection, unit: MovementUnit) -> CommandArgs {
    DeleteArgs {
        direction: Some(direction),
        unit,
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
        Motion::ByUnit(unit) => unit_to_str(unit),
        Motion::LineStep => "line-step",
        // PageStep 的 lines 通过 args.lines 另行携带，motion 字段仍是扁平字符串。
        Motion::PageStep { .. } => "page-step",
    }
}

/// 把 `MovementUnit` 序列化为 args 字符串——`motion_to_str` 与 `DeleteArgs` 共用。
fn unit_to_str(unit: MovementUnit) -> &'static str {
    match unit {
        MovementUnit::Grapheme => "grapheme",
        MovementUnit::Word => "word",
        MovementUnit::Identifier => "identifier",
        MovementUnit::Subword => "subword",
        MovementUnit::Symbol => "symbol",
        MovementUnit::LineEdge => "line-edge",
    }
}

/// 反向：仅接受 `MovementUnit` 枚举值，拒绝 `line-step` / `page-step`。
/// `DeleteArgs` 与 `MoveCaretArgs` 都用得到——后者再额外扩展到 Motion。
fn parse_unit(value: &str) -> Result<MovementUnit, CommandError> {
    match value {
        "grapheme" | "character" | "char" => Ok(MovementUnit::Grapheme),
        "word" => Ok(MovementUnit::Word),
        "identifier" => Ok(MovementUnit::Identifier),
        "subword" => Ok(MovementUnit::Subword),
        "symbol" => Ok(MovementUnit::Symbol),
        "line-edge" => Ok(MovementUnit::LineEdge),
        other => Err(CommandError::InvalidArgs(format!(
            "未知删除单位：{other}（删除不接受 line-step / page-step）",
        ))),
    }
}

fn parse_direction(value: &str) -> Result<MovementDirection, CommandError> {
    match value {
        "previous" | "left" => Ok(MovementDirection::Previous),
        "next" | "right" => Ok(MovementDirection::Next),
        other => Err(CommandError::InvalidArgs(format!("未知移动方向：{other}"))),
    }
}

fn parse_usize_arg(value: &str, name: &str) -> Result<usize, CommandError> {
    value
        .parse::<usize>()
        .map_err(|_| CommandError::InvalidArgs(format!("无效 {name}：{value}")))
}

fn parse_motion(value: &str, args: &CommandArgs) -> Result<Motion, CommandError> {
    match value {
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
        // 其余走 unit；parse_unit 不接受的字符串在那里报错。
        other => parse_unit(other)
            .map(Motion::ByUnit)
            .map_err(|_| CommandError::InvalidArgs(format!("未知光标运动：{other}"))),
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
    let merge_policy = context.edit_merge_policy;
    let mut target = context.edit_target()?;
    target.clear_visual_caret();
    let selections = target.selection.clone();
    let metadata = TransactionMetadata::new(TransactionSource::Programmatic)
        .with_merge_policy(merge_policy)
        .with_description("在选定位置插入");
    target
        .buffer
        .insert_at_selections(selections, &args.text, metadata)
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_replace_selection(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = ReplaceSelectionArgs::try_from(args)?;
    let mut target = context.edit_target()?;
    target.clear_visual_caret();
    let selections = target.selection.clone();
    target
        .buffer
        .replace_selections(
            selections,
            &args.text,
            TransactionMetadata::new(TransactionSource::Programmatic)
                .with_description("替换所选内容"),
        )
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_insert_newline(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let merge_policy = context.edit_merge_policy;
    let mut target = context.edit_target()?;
    target.clear_visual_caret();
    let selections = target.selection.clone();
    target
        .buffer
        .insert_at_selections(
            selections,
            "\n",
            TransactionMetadata::new(TransactionSource::Programmatic)
                .with_merge_policy(merge_policy)
                .with_description("插入换行"),
        )
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
    let mut target = context.edit_target()?;
    target.clear_visual_caret();
    let selections = target.selection.clone();
    target
        .buffer
        .outdent_at_selections(selections)
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_delete(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = DeleteArgs::try_from(args)?;
    let merge_policy = context.edit_merge_policy;
    let mut target = context.edit_target()?;
    target.clear_visual_caret();
    let selections = target.selection.clone();
    // caret_motion = Some → caret 沿方向删 unit、非空选区整段删；
    // caret_motion = None → caret no-op，仅删非空选区。
    let caret_motion = args.direction.map(|dir| (dir, args.unit));
    let description = delete_description(caret_motion);
    let metadata = TransactionMetadata::new(TransactionSource::Programmatic)
        .with_merge_policy(merge_policy)
        .with_description(description);
    target
        .buffer
        .delete_at_selections(selections, caret_motion, metadata)
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn delete_description(caret_motion: Option<(MovementDirection, MovementUnit)>) -> &'static str {
    match caret_motion {
        None => "删除所选内容",
        Some((MovementDirection::Previous, MovementUnit::Grapheme)) => "向后删除选定内容",
        Some((MovementDirection::Next, MovementUnit::Grapheme)) => "向前删除所选内容",
        Some((MovementDirection::Previous, MovementUnit::Word)) => "向后删除单词",
        Some((MovementDirection::Next, MovementUnit::Word)) => "向前删除单词",
        Some((MovementDirection::Previous, MovementUnit::Subword)) => "向后删除子词",
        Some((MovementDirection::Next, MovementUnit::Subword)) => "向前删除子词",
        Some((MovementDirection::Previous, MovementUnit::Identifier)) => "向后删除标识符",
        Some((MovementDirection::Next, MovementUnit::Identifier)) => "向前删除标识符",
        Some((MovementDirection::Previous, MovementUnit::Symbol)) => "向后删除符号",
        Some((MovementDirection::Next, MovementUnit::Symbol)) => "向前删除符号",
        Some((MovementDirection::Previous, MovementUnit::LineEdge)) => "删除到行首",
        Some((MovementDirection::Next, MovementUnit::LineEdge)) => "删除到行尾",
    }
}

fn run_select_all(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let mut target = context.edit_target()?;
    target.clear_visual_caret();
    let selection = SelectionSet::new(vec![Selection::new(
        ByteOffset::ZERO,
        target.buffer.len_bytes(),
    )]);
    target.set_selection(selection)?;
    Ok(CommandOutcome::default())
}

/// 把主编辑区"选区是否扩展"映射到 [`DismissScope::TextEdit`] 栈：
/// - 选区有 extent 且栈顶不是 [`CLEAR_SELECTION`] token → push 一条；
/// - 选区已塌（全是 caret）且栈顶就是 [`CLEAR_SELECTION`] token → pop。
///
/// 由 [`crate::commands::reconcile::after_dispatch`] 在每次 dispatch 末尾调一次。
/// 任何修改选区的命令（select_all / 带 shift 的 move / 文本编辑 / undo / redo / IME commit ……）都自动维护 esc 入口；
/// handler 自身不需要 push / pop。
///
/// 只看活动 view 的主选区——focused_field（搜索框 / 项目选择器输入框）即便也有"选区"也不参与，
/// 它们各自的瞬态由各自 scope（SearchInput / ProjectPicker）管理。
pub(crate) fn reconcile_text_edit_dismiss(context: &mut CommandContext<'_>) {
    let has_extent = context.active_view().is_some_and(|view| {
        view.selection()
            .as_slice()
            .iter()
            .any(|sel| !sel.is_caret())
    });
    let top_is_selection_token = context
        .dismiss
        .top_command_id(DismissScope::TextEdit)
        .is_some_and(|id| id.as_str() == CLEAR_SELECTION);

    match (has_extent, top_is_selection_token) {
        (true, false) => {
            context
                .dismiss
                .push(DismissScope::TextEdit, "取消选区", clear_selection());
        }
        (false, true) => {
            let _ = context.dismiss.pop_top(DismissScope::TextEdit);
        }
        _ => {}
    }
}

/// 把每个选区塌成 caret——head 不动，丢掉 anchor 形成的 extent；
/// 多 caret 保持不合并。
/// 选区本身就是 caret（无 extent）时 no-op。
fn run_clear_selection(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    {
        let mut target = context.edit_target()?;
        if target.selection.as_slice().iter().all(|sel| sel.is_caret()) {
            return Ok(CommandOutcome::default());
        }
        target.clear_visual_caret();
        let collapsed: Vec<Selection> = target
            .selection
            .as_slice()
            .iter()
            .map(|sel| Selection::caret(sel.head()))
            .collect();
        let primary_index = target.selection.primary_index();
        let next = SelectionSet::new_with_primary(collapsed, primary_index);
        target.set_selection(next)?;
    }
    Ok(CommandOutcome::default())
}

fn run_undo(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let mut target = context.edit_target()?;
    target.clear_visual_caret();
    target.buffer.undo().map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_redo(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let mut target = context.edit_target()?;
    target.clear_visual_caret();
    target.buffer.redo().map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_move_selection(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let mut args = MoveCaretArgs::try_from(args)?;
    // PageStep 步长按真实视口高度走：element 上一帧 prepaint 已把测得的 visible_visual_rows 写回 ViewportState，从这里读。
    // focused_field 模式下作用于输入框（通常单行），主编辑区的视口高度对它无意义，保留 keymap 兜底。
    // visible_visual_rows == 0（首帧 / headless）也走兜底。
    if let Motion::PageStep { lines } = &mut args.motion
        && context.focused_field.is_none()
        && let Some(view) = context.active_view()
    {
        let measured = view.viewport().visible_visual_rows;
        if measured > 0 {
            *lines = u32::try_from((measured * 2 / 3).max(1)).unwrap_or(u32::MAX);
        }
    }
    let extend = args.extend;
    let target = context.edit_target()?;
    let selections = target.selection.clone();
    // 非扩展移动且有选区时，先塌缩到方向边缘，再交给下游移动。
    // 这样 engine / visual_movement 只看到简单 caret，不需要各自处理塌缩——换行边界、视觉行移动等路径一个都不用改。
    let had_extent = !extend && selections.as_slice().iter().any(|sel| !sel.is_caret());
    let selections = if had_extent {
        let primary_index = selections.primary_index();
        let collapsed: Vec<Selection> = selections
            .as_slice()
            .iter()
            .map(|sel| {
                Selection::caret(match args.direction {
                    MovementDirection::Previous => sel.start(),
                    MovementDirection::Next => sel.end(),
                })
            })
            .collect();
        SelectionSet::new_with_primary(collapsed, primary_index)
    } else {
        selections
    };
    move_target_selection(target, selections, args.direction, args.motion, extend)?;
    Ok(CommandOutcome::default())
}

fn run_ime_commit(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = ImeCommitArgs::try_from(args)?;
    let mut target = context.edit_target()?;
    target.clear_visual_caret();
    apply_ime_replacement_range(&mut target, args.replacement_range_utf16)?;
    target
        .buffer
        .set_selection(target.selection.clone())
        .map_err(command_execution_failed)?;
    target
        .buffer
        .commit_composition(&args.text)
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_ime_update(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = ImeUpdateArgs::try_from(args)?;
    let mut target = context.edit_target()?;
    target.clear_visual_caret();
    apply_ime_replacement_range(&mut target, args.replacement_range_utf16)?;

    // 系统输入法把 marked text 置空 = 放弃组合（如按 Esc 取消候选）。
    // 必须真正结束 composition，避免系统 IME 继续认为组合会话仍在。
    if args.text.is_empty() {
        if target.buffer.is_composing() {
            target
                .buffer
                .cancel_composition()
                .map_err(command_execution_failed)?;
            *target.selection = target.buffer.selection().clone();
        }
        return Ok(CommandOutcome::default());
    }

    let relative_selection = match args.selected_range_utf16 {
        Some(range) => Some(composition_selection_from_utf16(&args.text, range)?),
        None => None,
    };
    target
        .buffer
        .update_composition(&args.text, relative_selection)
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn run_ime_cancel(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let mut target = context.edit_target()?;
    target.clear_visual_caret();
    target
        .buffer
        .cancel_composition()
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

fn apply_ime_replacement_range(
    target: &mut crate::EditTarget<'_>,
    replacement_range_utf16: Option<ImeUtf16RangeArgs>,
) -> Result<(), CommandError> {
    let Some(range_utf16) = replacement_range_utf16 else {
        return Ok(());
    };
    if target.buffer.is_composing() {
        return Ok(());
    }
    let range = ime_range_to_text_range(target.buffer, range_utf16)?;
    let selection = SelectionSet::new(vec![Selection::new(range.start(), range.end())]);
    target.set_selection(selection)
}

fn ime_range_to_text_range(
    buffer: &Buffer,
    range_utf16: ImeUtf16RangeArgs,
) -> Result<TextRange, CommandError> {
    let start = buffer
        .utf16_cu_to_byte(Utf16Offset::new(range_utf16.start))
        .map_err(|_| CommandError::InvalidArgs("IME range start 越界".into()))?;
    let end = buffer
        .utf16_cu_to_byte(Utf16Offset::new(range_utf16.end))
        .map_err(|_| CommandError::InvalidArgs("IME range end 越界".into()))?;

    TextRange::new(start, end)
        .map_err(|_| CommandError::InvalidArgs("IME range start 大于 end".into()))
}

fn composition_selection_from_utf16(
    preedit: &str,
    range_utf16: ImeUtf16RangeArgs,
) -> Result<CompositionSelection, CommandError> {
    let anchor = utf16_to_byte_offset_in_str(preedit, range_utf16.start)
        .ok_or_else(|| CommandError::InvalidArgs("IME preedit selection anchor 越界".into()))?;
    let head = utf16_to_byte_offset_in_str(preedit, range_utf16.end)
        .ok_or_else(|| CommandError::InvalidArgs("IME preedit selection head 越界".into()))?;
    Ok(CompositionSelection::new(
        ByteOffset::new(anchor),
        ByteOffset::new(head),
    ))
}

fn utf16_to_byte_offset_in_str(text: &str, target: usize) -> Option<usize> {
    if target == 0 {
        return Some(0);
    }
    let mut utf16 = 0usize;
    for (idx, ch) in text.char_indices() {
        let step = ch.len_utf16();
        if utf16 + step > target {
            return None;
        }
        utf16 += step;
        if utf16 == target {
            return Some(idx + ch.len_utf8());
        }
    }
    if utf16 == target {
        Some(text.len())
    } else {
        None
    }
}

fn run_ime_confirm(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let mut target = context.edit_target()?;
    target.clear_visual_caret();
    let Some(preedit) = target
        .buffer
        .composition()
        .map(|state| state.preedit_text().to_string())
    else {
        return Ok(CommandOutcome::default());
    };
    target
        .buffer
        .set_selection(target.selection.clone())
        .map_err(command_execution_failed)?;
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
    // 导航需要同时感知编辑 tab 与预览 tab，而 CommandContext 只持有 ViewSet。
    // 把方向交给宿主，由宿主在完整 session 上完成跨类型的循环导航。
    let forward = matches!(args.target, SelectTabTarget::Next);
    context
        .effects
        .push(HostEffect::EditorSelectAdjacentTab(forward));
    Ok(CommandOutcome::default())
}

fn run_close_tab(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let view_id = if args.is_empty() {
        // 键盘快捷键路径：keymap 绑定时无参，从 context 取当前活动 view。
        context
            .active_view_id
            .ok_or_else(|| CommandError::InvalidArgs("没有活动标签可关闭".into()))?
    } else {
        // 点击标签关闭 glyph 路径：args 含序列化的 ViewId。
        reject_unknown_args(&args, &["view_id"])?;
        let raw = required_arg(&args, "view_id")?;
        let id: u64 = raw
            .parse()
            .map_err(|_| CommandError::InvalidArgs(format!("无效的 view_id：{raw}")))?;
        ViewId::from_u64(id)
    };
    context.effects.push(HostEffect::EditorCloseTab(view_id));
    Ok(CommandOutcome::default())
}

fn run_save(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    let buffer_id = active_view_buffer_id(context)?;
    if let Err(error) = context.workspace.save_file(buffer_id) {
        context.effects.push(HostEffect::ShowBubble(
            BubbleRequest::error(format!("保存失败：{error}")).dedupe("editor.save"),
        ));
    }
    Ok(CommandOutcome::default())
}

/// 切换软换行：emit 一个 `HostEffect`，宿主侧翻转 EditorKernel 的 soft_wrap 状态。
/// command 层不持有渲染 kernel，所以走 effect 让 desktop 自己决定具体翻哪个。
fn run_toggle_soft_wrap(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    if let Some(view) = context.active_view_mut() {
        view.clear_visual_caret();
    }
    context.effects.push(HostEffect::EditorToggleSoftWrap);
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
    // 先取出剪贴板文本 + 删除材料；过程中只借 target，借完即释放，之后才碰 `context.clipboard` 写入，避免对 context 的两次可变借用相撞。
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

    let mut target = context.edit_target()?;
    target.clear_visual_caret();
    // cut 两条分支都是「调用方先把要删的 range 算好了」——caret_motion=None
    // 让引擎跳过 caret 处理，整段删传入的非空 selection 即可。
    match plan {
        CutPlan::DeleteSelections(selections) => {
            target
                .buffer
                .delete_at_selections(
                    selections,
                    None,
                    TransactionMetadata::new(TransactionSource::Programmatic)
                        .with_description("剪切所选内容"),
                )
                .map_err(command_execution_failed)?;
        }
        CutPlan::DeleteLineRanges(line_ranges) => {
            if !line_ranges.is_empty() {
                let line_selections = SelectionSet::from_ranges(line_ranges);
                target
                    .buffer
                    .delete_at_selections(
                        line_selections,
                        None,
                        TransactionMetadata::new(TransactionSource::Programmatic)
                            .with_description("剪切行"),
                    )
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
    let mut target = context.edit_target()?;
    target.clear_visual_caret();
    let selections = target.selection.clone();
    target
        .buffer
        .replace_selections(
            selections,
            &text,
            TransactionMetadata::new(TransactionSource::Programmatic).with_description("粘贴"),
        )
        .map_err(command_execution_failed)?;
    *target.selection = target.buffer.selection().clone();
    Ok(CommandOutcome::default())
}

/// 收集要写入剪贴板的字符串。
///
/// 二选一规则：
/// - 任意非空 selection 存在 → 只取非空段，`\n` 分隔；
/// - 全部 caret → 每个 caret 取所在行（含 `\n`），按 [`Line`] 去重并按行号顺序，直接拼接（行文本已自带 `\n`，末行无 `\n` 时不补）。
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
