//! M7A / M7B / M7C：编辑命令数据模型、命令上下文与执行器。
//!
//! `Command` 是键盘、鼠标、菜单、命令面板、GPUI testbed 与未来宏录制的统一
//! 动作描述入口。M7A 定义命令枚举；M7B 补充命令来源、repeat、timestamp 与
//! Transaction metadata 映射；M7C 把命令统一执行到 Buffer 的 Transaction、Selection、
//! History 与 Composition 管线。可回放序列化能力留给 M17。

use std::time::SystemTime;

use crate::{
    BufferVersion, CharOffset, EngineResult, Line, LogicalColumn, MovementDirection, MovementUnit,
    Position, Selection, SelectionSet, TransactionId, TransactionMetadata, TransactionSource,
    buffer::Buffer,
};

/// 统一编辑动作枚举。
///
/// 该类型描述“用户或宿主想做什么”。M7C 的 `Buffer::execute_command`
/// 会负责把文本修改命令落到 `Transaction`，把移动命令落到 `SelectionSet`，
/// 并复用 M6C 的 composition 管线。
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

    /// 根据命令类型和来源，给未来文本事务选择合适的 `TransactionSource`。
    ///
    /// 纯移动命令在 M7C 中不会产生文本事务；这个方法仍然保持确定性，方便 testbed
    /// 统一展示“如果该命令产生事务，将使用什么来源”。
    pub fn transaction_source(&self, source: CommandSource) -> TransactionSource {
        match self {
            Self::Undo => TransactionSource::Undo,
            Self::Redo => TransactionSource::Redo,
            Self::CompositionStart
            | Self::CompositionUpdate(_)
            | Self::CompositionCommit(_)
            | Self::CompositionCancel => TransactionSource::Composition,
            Self::DeleteSelection
            | Self::DeleteBackward
            | Self::DeleteForward
            | Self::DeleteWordBackward
            | Self::DeleteWordForward
            | Self::DeleteSubwordBackward
            | Self::DeleteSubwordForward => TransactionSource::Delete,
            _ => source.into(),
        }
    }
}

/// 命令来源。
///
/// `CommandSource` 描述“命令从哪里进入引擎”。它不同于 `TransactionSource`：
/// 前者是输入来源，后者是事务语义标签。M7B 提供二者的显式映射，M7C 执行命令时
/// 可以用该映射生成 `TransactionMetadata`。
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

impl From<CommandSource> for TransactionSource {
    fn from(source: CommandSource) -> Self {
        match source {
            CommandSource::Keyboard => Self::Keyboard,
            CommandSource::Mouse => Self::Mouse,
            CommandSource::Paste => Self::Paste,
            CommandSource::CommandPalette | CommandSource::Menu => Self::Command,
            CommandSource::Ime => Self::Composition,
            CommandSource::Macro => Self::Macro,
            CommandSource::External => Self::External,
        }
    }
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
/// M7B 只承载上下文数据，不执行命令。后续 M7C 的执行器会读取这里的来源、repeat
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

    /// 为未来的文本事务生成基础 metadata。
    ///
    /// 这里不提交事务，也不决定 Undo merge；只把 M7B 的来源模型映射为现有
    /// `TransactionMetadata`，并填入稳定命令描述。
    pub fn transaction_metadata_for(&self, command: &Command) -> TransactionMetadata {
        TransactionMetadata::new(command.transaction_source(self.source))
            .with_description(command.description())
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
/// M7C 先暴露稳定的执行摘要：版本是否变化、selection / composition 是否变化、
/// 以及用于状态栏 / testbed 的短描述。当前 Transaction 系统尚未分配稳定
/// `TransactionId`，因此 `transaction_id` 暂时为 `None`，字段先保留为后续 M9
/// DeltaEvent / 事务事件总线对接点。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommandOutcome {
    pub old_version: BufferVersion,
    pub new_version: BufferVersion,
    pub transaction_id: Option<TransactionId>,
    pub text_changed: bool,
    pub selection_changed: bool,
    pub composition_changed: bool,
    pub description: String,
}

impl CommandOutcome {
    fn new(
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

impl Buffer {
    /// M7C：统一命令执行入口。
    ///
    /// 文本修改命令会走现有 Transaction 管线；纯移动命令只更新 `SelectionSet`，
    /// 不递增文本版本；Undo / Redo 复用历史系统；IME 命令复用 M6C composition 管线。
    pub fn execute_command(
        &mut self,
        command: Command,
        context: CommandContext,
    ) -> EngineResult<CommandOutcome> {
        let old_version = self.version();
        let old_selection = self.selection().clone();
        let old_composition = self.composition().cloned();
        let description = command.description();

        for _ in 0..context.repeat_count() {
            self.execute_command_once(&command, &context)?;
        }

        let new_version = self.version();
        let selection_changed = self.selection() != &old_selection;
        let composition_changed = self.composition().cloned() != old_composition;

        Ok(CommandOutcome::new(
            old_version,
            new_version,
            None,
            selection_changed,
            composition_changed,
            description,
        ))
    }

    fn execute_command_once(
        &mut self,
        command: &Command,
        context: &CommandContext,
    ) -> EngineResult<()> {
        match command {
            Command::InsertText(text) | Command::ReplaceSelections(text) => {
                let selections = self.selection().clone();
                self.replace_selection_ranges_with_metadata(
                    selections,
                    text,
                    context.transaction_metadata_for(command),
                )?;
            }
            Command::DeleteSelection => {
                let selections = self.selection().clone();
                self.replace_selection_ranges_with_metadata(
                    selections,
                    "",
                    context.transaction_metadata_for(command),
                )?;
            }
            Command::DeleteBackward => {
                self.delete_by_movement(
                    MovementDirection::Previous,
                    MovementUnit::Grapheme,
                    context.transaction_metadata_for(command),
                )?;
            }
            Command::DeleteForward => {
                self.delete_by_movement(
                    MovementDirection::Next,
                    MovementUnit::Grapheme,
                    context.transaction_metadata_for(command),
                )?;
            }
            Command::DeleteWordBackward => {
                self.delete_by_movement(
                    MovementDirection::Previous,
                    MovementUnit::Word,
                    context.transaction_metadata_for(command),
                )?;
            }
            Command::DeleteWordForward => {
                self.delete_by_movement(
                    MovementDirection::Next,
                    MovementUnit::Word,
                    context.transaction_metadata_for(command),
                )?;
            }
            Command::DeleteSubwordBackward => {
                self.delete_by_movement(
                    MovementDirection::Previous,
                    MovementUnit::Subword,
                    context.transaction_metadata_for(command),
                )?;
            }
            Command::DeleteSubwordForward => {
                self.delete_by_movement(
                    MovementDirection::Next,
                    MovementUnit::Subword,
                    context.transaction_metadata_for(command),
                )?;
            }
            Command::MoveLeft { extend } => {
                self.move_current_selection(
                    MovementDirection::Previous,
                    MovementUnit::Grapheme,
                    *extend,
                )?;
            }
            Command::MoveRight { extend } => {
                self.move_current_selection(
                    MovementDirection::Next,
                    MovementUnit::Grapheme,
                    *extend,
                )?;
            }
            Command::MoveUp { extend } => {
                self.move_current_selection_vertically(MovementDirection::Previous, *extend)?;
            }
            Command::MoveDown { extend } => {
                self.move_current_selection_vertically(MovementDirection::Next, *extend)?;
            }
            Command::MoveLineStart { extend } => {
                self.move_current_selection_to_line_edge(LineEdge::Start, *extend)?;
            }
            Command::MoveLineEnd { extend } => {
                self.move_current_selection_to_line_edge(LineEdge::End, *extend)?;
            }
            Command::MoveWordLeft { extend } => {
                self.move_current_selection(
                    MovementDirection::Previous,
                    MovementUnit::Word,
                    *extend,
                )?;
            }
            Command::MoveWordRight { extend } => {
                self.move_current_selection(MovementDirection::Next, MovementUnit::Word, *extend)?;
            }
            Command::MoveSubwordLeft { extend } => {
                self.move_current_selection(
                    MovementDirection::Previous,
                    MovementUnit::Subword,
                    *extend,
                )?;
            }
            Command::MoveSubwordRight { extend } => {
                self.move_current_selection(
                    MovementDirection::Next,
                    MovementUnit::Subword,
                    *extend,
                )?;
            }
            Command::MoveSymbolLeft { extend } => {
                self.move_current_selection(
                    MovementDirection::Previous,
                    MovementUnit::Symbol,
                    *extend,
                )?;
            }
            Command::MoveSymbolRight { extend } => {
                self.move_current_selection(
                    MovementDirection::Next,
                    MovementUnit::Symbol,
                    *extend,
                )?;
            }
            Command::SelectAll => {
                self.set_selection(SelectionSet::new(vec![Selection::new(
                    CharOffset::ZERO,
                    self.len_chars(),
                )]))?;
            }
            Command::ClearSelections => {
                let head = self.selection().primary().head();
                self.set_selection(SelectionSet::caret(head))?;
            }
            Command::SetSelections(selections) => {
                self.set_selection(selections.clone())?;
            }
            Command::AddSelection(selection) => {
                let mut selections = self.selection().as_slice().to_vec();
                let primary_index = selections.len();
                selections.push(*selection);
                self.set_selection(SelectionSet::new_with_primary(selections, primary_index))?;
            }
            Command::Undo => {
                self.undo()?;
            }
            Command::Redo => {
                self.redo()?;
            }
            Command::CompositionStart => {
                self.start_composition()?;
            }
            Command::CompositionUpdate(preedit_text) => {
                self.update_composition(preedit_text, None)?;
            }
            Command::CompositionCommit(commit_text) => {
                self.commit_composition(commit_text)?;
            }
            Command::CompositionCancel => {
                self.cancel_composition()?;
            }
        }

        Ok(())
    }

    fn delete_by_movement(
        &mut self,
        direction: MovementDirection,
        unit: MovementUnit,
        metadata: TransactionMetadata,
    ) -> EngineResult<()> {
        let selections = self.selection().clone();
        let mut delete_targets = Vec::new();

        for selection in selections.as_slice() {
            if selection.is_caret() {
                let head = selection.head();
                let boundary = self.movement_boundary(head, direction, unit)?;
                let range_selection = match direction {
                    MovementDirection::Previous => Selection::new(boundary, head),
                    MovementDirection::Next => Selection::new(head, boundary),
                };

                if !range_selection.range().is_empty() {
                    delete_targets.push(range_selection);
                }
            } else {
                delete_targets.push(*selection);
            }
        }

        if delete_targets.is_empty() {
            self.set_selection(selections)?;
            return Ok(());
        }

        self.replace_selection_ranges_with_metadata(
            SelectionSet::new(delete_targets),
            "",
            metadata,
        )?;
        Ok(())
    }

    fn move_current_selection_to_line_edge(
        &mut self,
        edge: LineEdge,
        extend: bool,
    ) -> EngineResult<SelectionSet> {
        let selections = self.selection().clone();
        let primary_index = selections.primary_index();
        let moved = selections
            .as_slice()
            .iter()
            .copied()
            .map(|selection| {
                let position = self.char_to_position(selection.head())?;
                let new_head = match edge {
                    LineEdge::Start => self.line_start(position.line())?,
                    LineEdge::End => self.line_content_end(position.line())?,
                };

                Ok(if extend {
                    selection.with_head(new_head)
                } else {
                    Selection::caret(new_head)
                })
            })
            .collect::<EngineResult<Vec<_>>>()?;

        let moved = SelectionSet::new_with_primary(moved, primary_index);
        self.set_selection(moved.clone())?;
        Ok(moved)
    }

    fn move_current_selection_vertically(
        &mut self,
        direction: MovementDirection,
        extend: bool,
    ) -> EngineResult<SelectionSet> {
        let selections = self.selection().clone();
        let primary_index = selections.primary_index();
        let moved = selections
            .as_slice()
            .iter()
            .copied()
            .map(|selection| {
                let new_head = self.vertical_movement_target(selection.head(), direction)?;
                Ok(if extend {
                    selection.with_head(new_head)
                } else {
                    Selection::caret(new_head)
                })
            })
            .collect::<EngineResult<Vec<_>>>()?;

        let moved = SelectionSet::new_with_primary(moved, primary_index);
        self.set_selection(moved.clone())?;
        Ok(moved)
    }

    fn vertical_movement_target(
        &self,
        offset: CharOffset,
        direction: MovementDirection,
    ) -> EngineResult<CharOffset> {
        let position = self.char_to_position(offset)?;
        let current_line = position.line().get();
        let last_line = self.line_count().saturating_sub(1);
        let target_line = match direction {
            MovementDirection::Previous => current_line.saturating_sub(1),
            MovementDirection::Next => (current_line + 1).min(last_line),
        };

        if target_line == current_line {
            return Ok(offset);
        }

        let target_line = Line::new(target_line);
        let target_line_start = self.line_start(target_line)?;
        let target_line_end = self.line_content_end(target_line)?;
        let target_line_len = target_line_end
            .get()
            .saturating_sub(target_line_start.get());
        let target_column = position.column().get().min(target_line_len);

        self.position_to_char(Position::new(
            target_line,
            LogicalColumn::new(target_column),
        ))
    }

    fn line_content_end(&self, line: Line) -> EngineResult<CharOffset> {
        let next_line = line.get() + 1;
        if next_line >= self.line_count() {
            return Ok(self.len_chars());
        }

        let next_line_start = self.line_start(Line::new(next_line))?;
        self.previous_grapheme_boundary(next_line_start)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineEdge {
    Start,
    End,
}
