//! M7A～M7D：编辑命令数据模型、命令上下文与执行结果。
//!
//! `Command` 是键盘、鼠标、菜单、命令面板、GPUI testbed 与未来宏录制的统一
//! 动作描述入口。M7A 定义命令枚举；M7B 补充命令来源、repeat 与 timestamp；
//! M7C/M7D 的执行器位于 `src/command_executor.rs`，负责把命令
//! 统一编排到 Buffer 的 Transaction、SelectionSet、History 与 Composition 管线。
//! 可回放序列化能力留给 M17。

use std::time::SystemTime;

use crate::{BufferVersion, Selection, SelectionSet, TransactionId};

/// 统一编辑动作枚举。
///
/// 该类型描述“用户或宿主想做什么”。`src/command_executor.rs` 中的
/// `Buffer::execute_command` 会负责把文本修改命令落到 `Transaction`，
/// 把移动命令落到 `SelectionSet`，并复用 M6C 的 composition 管线。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Command {
    // ---- 文本输入 ----
    /// 在当前选区处插入文本；非空选区会被替换。
    InsertText(String),
    /// 用同一段文本替换所有当前选区。
    ReplaceSelections(String),
    /// 删除当前非空选区；caret 本身不删除字符。
    DeleteSelection,

    // ---- 删除 ----
    /// 向后删除一个用户感知字符；非空选区直接删除选区。
    DeleteBackward,
    /// 向前删除一个用户感知字符；非空选区直接删除选区。
    DeleteForward,
    /// 向后删除一个 Unicode word。
    DeleteWordBackward,
    /// 向前删除一个 Unicode word。
    DeleteWordForward,
    /// 向后删除一个 identifier subword。
    DeleteSubwordBackward,
    /// 向前删除一个 identifier subword。
    DeleteSubwordForward,

    // ---- 基础移动 ----
    MoveLeft {
        extend: bool,
    },
    MoveRight {
        extend: bool,
    },
    MoveUp {
        extend: bool,
    },
    MoveDown {
        extend: bool,
    },
    MoveLineStart {
        extend: bool,
    },
    MoveLineEnd {
        extend: bool,
    },

    // ---- word / subword / symbol 移动 ----
    MoveWordLeft {
        extend: bool,
    },
    MoveWordRight {
        extend: bool,
    },
    MoveSubwordLeft {
        extend: bool,
    },
    MoveSubwordRight {
        extend: bool,
    },
    MoveSymbolLeft {
        extend: bool,
    },
    MoveSymbolRight {
        extend: bool,
    },

    // ---- SelectionSet ----
    SelectAll,
    ClearSelections,
    SetSelections(SelectionSet),
    AddSelection(Selection),

    // ---- 历史 ----
    Undo,
    Redo,

    // ---- IME composition ----
    CompositionStart,
    CompositionUpdate(String),
    CompositionCommit(String),
    CompositionCancel,
}

impl Command {
    pub fn insert_text(text: impl Into<String>) -> Self {
        Self::InsertText(text.into())
    }

    pub fn replace_selections(text: impl Into<String>) -> Self {
        Self::ReplaceSelections(text.into())
    }

    pub fn composition_update(preedit_text: impl Into<String>) -> Self {
        Self::CompositionUpdate(preedit_text.into())
    }

    pub fn composition_commit(commit_text: impl Into<String>) -> Self {
        Self::CompositionCommit(commit_text.into())
    }

    /// 返回稳定、短文本的命令描述，供 Undo / Redo 列表、状态栏与 testbed 展示使用。
    ///
    /// 这里故意不包含用户输入的具体文本，避免状态栏暴露过长内容，也避免未来宏录制
    /// 或日志中意外保存大段文本。更详细的描述可以由 M7C 的 `CommandOutcome` 扩展。
    pub fn description(&self) -> &'static str {
        match self {
            Self::InsertText(_) => "insert text",
            Self::ReplaceSelections(_) => "replace selections",
            Self::DeleteSelection => "delete selection",
            Self::DeleteBackward => "delete backward",
            Self::DeleteForward => "delete forward",
            Self::DeleteWordBackward => "delete word backward",
            Self::DeleteWordForward => "delete word forward",
            Self::DeleteSubwordBackward => "delete subword backward",
            Self::DeleteSubwordForward => "delete subword forward",
            Self::MoveLeft { extend: false } => "move left",
            Self::MoveLeft { extend: true } => "extend left",
            Self::MoveRight { extend: false } => "move right",
            Self::MoveRight { extend: true } => "extend right",
            Self::MoveUp { extend: false } => "move up",
            Self::MoveUp { extend: true } => "extend up",
            Self::MoveDown { extend: false } => "move down",
            Self::MoveDown { extend: true } => "extend down",
            Self::MoveLineStart { extend: false } => "move line start",
            Self::MoveLineStart { extend: true } => "extend line start",
            Self::MoveLineEnd { extend: false } => "move line end",
            Self::MoveLineEnd { extend: true } => "extend line end",
            Self::MoveWordLeft { extend: false } => "move word left",
            Self::MoveWordLeft { extend: true } => "extend word left",
            Self::MoveWordRight { extend: false } => "move word right",
            Self::MoveWordRight { extend: true } => "extend word right",
            Self::MoveSubwordLeft { extend: false } => "move subword left",
            Self::MoveSubwordLeft { extend: true } => "extend subword left",
            Self::MoveSubwordRight { extend: false } => "move subword right",
            Self::MoveSubwordRight { extend: true } => "extend subword right",
            Self::MoveSymbolLeft { extend: false } => "move symbol left",
            Self::MoveSymbolLeft { extend: true } => "extend symbol left",
            Self::MoveSymbolRight { extend: false } => "move symbol right",
            Self::MoveSymbolRight { extend: true } => "extend symbol right",
            Self::SelectAll => "select all",
            Self::ClearSelections => "clear selections",
            Self::SetSelections(_) => "set selections",
            Self::AddSelection(_) => "add selection",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::CompositionStart => "composition start",
            Self::CompositionUpdate(_) => "composition update",
            Self::CompositionCommit(_) => "composition commit",
            Self::CompositionCancel => "composition cancel",
        }
    }
}

/// 命令来源。
///
/// `CommandSource` 描述“命令从哪里进入引擎”。它只表达输入来源；
/// Command -> Transaction metadata 的映射属于 `src/command_executor.rs` 的适配层职责。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CommandSource {
    #[default]
    Keyboard,
    Mouse,
    Paste,
    CommandPalette,
    Menu,
    Ime,
    Macro,
    External,
}

/// 命令 repeat 次数。
///
/// 0 次 repeat 没有编辑语义，因此构造器会拒绝 0。默认值为 1。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandRepeat(u32);

impl CommandRepeat {
    pub const DEFAULT: Self = Self(1);

    pub const fn new(value: u32) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

impl Default for CommandRepeat {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// 命令执行上下文。
///
/// M7B 只承载上下文数据，不直接执行命令。M7C/M7D 的执行器会读取这里的来源、repeat
/// 和 timestamp，并把文本修改命令转换为 Transaction。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommandContext {
    source: CommandSource,
    repeat: CommandRepeat,
    timestamp: Option<SystemTime>,
}

impl CommandContext {
    pub fn new(source: CommandSource) -> Self {
        Self {
            source,
            ..Self::default()
        }
    }

    pub fn keyboard() -> Self {
        Self::new(CommandSource::Keyboard)
    }

    pub fn mouse() -> Self {
        Self::new(CommandSource::Mouse)
    }

    pub fn paste() -> Self {
        Self::new(CommandSource::Paste)
    }

    pub fn command_palette() -> Self {
        Self::new(CommandSource::CommandPalette)
    }

    pub fn menu() -> Self {
        Self::new(CommandSource::Menu)
    }

    pub fn ime() -> Self {
        Self::new(CommandSource::Ime)
    }

    pub fn macro_replay() -> Self {
        Self::new(CommandSource::Macro)
    }

    pub fn external() -> Self {
        Self::new(CommandSource::External)
    }

    pub fn with_repeat(mut self, repeat: CommandRepeat) -> Self {
        self.repeat = repeat;
        self
    }

    pub fn with_repeat_count(self, repeat: u32) -> Option<Self> {
        Some(self.with_repeat(CommandRepeat::new(repeat)?))
    }

    pub fn with_timestamp(mut self, timestamp: SystemTime) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    pub const fn source(&self) -> CommandSource {
        self.source
    }

    pub const fn repeat(&self) -> CommandRepeat {
        self.repeat
    }

    pub const fn repeat_count(&self) -> u32 {
        self.repeat.get()
    }

    pub fn timestamp(&self) -> Option<SystemTime> {
        self.timestamp
    }
}

impl Default for CommandContext {
    fn default() -> Self {
        Self {
            source: CommandSource::Keyboard,
            repeat: CommandRepeat::DEFAULT,
            timestamp: None,
        }
    }
}

/// 命令执行结果。
///
/// M7C/M7D 暴露稳定的执行摘要：版本是否变化、selection / composition 是否变化、
/// 以及用于状态栏 / testbed 的短描述。当前 Transaction 系统尚未分配稳定
/// `TransactionId`，因此 `transaction_id` 暂时为 `None`，字段先保留为后续 M9
/// DeltaEvent / 事务事件总线对接点。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommandOutcome {
    old_version: BufferVersion,
    new_version: BufferVersion,
    transaction_id: Option<TransactionId>,
    text_changed: bool,
    selection_changed: bool,
    composition_changed: bool,
    description: String,
}

impl CommandOutcome {
    pub(crate) fn new(
        old_version: BufferVersion,
        new_version: BufferVersion,
        transaction_id: Option<TransactionId>,
        selection_changed: bool,
        composition_changed: bool,
        description: impl Into<String>,
    ) -> Self {
        Self {
            old_version,
            new_version,
            transaction_id,
            text_changed: old_version != new_version,
            selection_changed,
            composition_changed,
            description: description.into(),
        }
    }

    pub const fn old_version(&self) -> BufferVersion {
        self.old_version
    }

    pub const fn new_version(&self) -> BufferVersion {
        self.new_version
    }

    pub const fn transaction_id(&self) -> Option<TransactionId> {
        self.transaction_id
    }

    pub const fn text_changed(&self) -> bool {
        self.text_changed
    }

    pub const fn selection_changed(&self) -> bool {
        self.selection_changed
    }

    pub const fn composition_changed(&self) -> bool {
        self.composition_changed
    }

    pub fn description(&self) -> &str {
        &self.description
    }
}
