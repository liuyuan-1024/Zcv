//! zom-command —— 命令派发脊柱 + 键位模型
//!
//! 「所有操作均是命令」。键盘、命令面板、AI、菜单都把意图收敛成
//! `(CommandId, CommandArgs)`，经唯一派发路径进入执行器（参数模型方案 A）。
//!
//! 依赖 `zom-workspace` / `zom-view` / `zom-engine`（它要编辑的东西），
//! **不依赖** `zom-ai` 等扩展域 —— 扩展域的命令由 `zom-desktop` 组合根
//! 注册（handler 闭包捕获扩展服务）。
//!
//! ## 模块划分
//! - 本文件：核心机制（`CommandId / CommandArgs / Registry / Executor /
//!   Keymap / KeyBinding`）以及通用错误。**不**直接持有任何具体命令。
//! - [`commands`]：按域分组的"命令目录"。每个域一个子模块，**同处**声明：
//!   常量 id、typed args、typed builders、handler、默认键位。注册和调用
//!   都走这里，调用方不再到处手拼字符串。

use std::collections::{BTreeMap, VecDeque};
use std::fmt;

use zom_engine::EngineError;
use zom_view::ViewSet;
use zom_workspace::{BufferId, Workspace};

pub mod commands;
pub mod effects;
pub mod keymap_format;

pub use effects::{EffectQueue, HostEffect};

/// 一次命令调用所需的两个组件，等价于"未提交的派发请求"。
///
/// 各 catalog 的 typed builders（如 `editor::insert_text(...)`）都返回此别名 ——
/// 调用方拿到后只需 `app.dispatch(invocation)`，无需再手拼 id 字符串或 args。
pub type Invocation = (CommandId, CommandArgs);

// ===== 命令标识与参数 =====

/// 命令的稳定标识，例如 `editor.insert_text`。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CommandId(String);

impl CommandId {
    pub fn new(value: impl Into<String>) -> Result<Self, CommandError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(CommandError::InvalidCommandId);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CommandId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 命令参数的不透明载荷（方案 A）。
///
/// 派发边界统一为此类型，每条命令在自己的目录模块里通过 `TryFrom<CommandArgs>`
/// 解析成强类型参数，构造侧则通过 `From<TypedArgs>` 反向生成 ——
/// 字段名只在命令模块出现一次。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandArgs {
    fields: BTreeMap<String, String>,
}

impl CommandArgs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.insert(key.into(), value.into());
        self
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.get(key).map(String::as_str)
    }

    pub fn fields(&self) -> impl Iterator<Item = (&str, &str)> {
        self.fields
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// 不接受参数的命令参数约定。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NoArgs;

impl TryFrom<CommandArgs> for NoArgs {
    type Error = CommandError;

    fn try_from(args: CommandArgs) -> Result<Self, Self::Error> {
        if args.is_empty() {
            Ok(Self)
        } else {
            Err(CommandError::InvalidArgs(format!(
                "该命令不接受参数：{}",
                format_arg_keys(&args)
            )))
        }
    }
}

impl From<NoArgs> for CommandArgs {
    fn from(_: NoArgs) -> Self {
        CommandArgs::new()
    }
}

// ===== 命令与注册表 =====

/// 命令的元数据。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Command {
    pub id: CommandId,
    pub title: String,
}

impl Command {
    pub fn new(id: CommandId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
        }
    }
}

/// 命令 handler：对 `CommandContext` 执行一次操作。
///
/// 内建编辑命令在 [`commands::editor`] 注册；`ai.*`、未来 lsp/git 由
/// `zom-desktop` 注册 —— 闭包捕获扩展服务，扩展域不进 `CommandContext`。
pub type CommandHandler =
    Box<dyn Fn(&mut CommandContext<'_>, CommandArgs) -> Result<CommandOutcome, CommandError>>;

/// 类型擦除的开放注册表：`CommandId -> (元数据, handler)`。
#[derive(Default)]
pub struct CommandRegistry {
    commands: BTreeMap<CommandId, (Command, CommandHandler)>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一条命令。重复 id 返回错误。
    ///
    /// 一般 catalog 不直接调本方法 —— 用 [`Self::install`] 更短，
    /// 而且能在同一个表达式里继续绑默认键位。
    pub fn register(
        &mut self,
        command: Command,
        handler: CommandHandler,
    ) -> Result<(), CommandError> {
        if self.commands.contains_key(&command.id) {
            return Err(CommandError::DuplicateCommand(command.id));
        }
        self.commands.insert(command.id.clone(), (command, handler));
        Ok(())
    }

    /// 注册一条命令并返回 [`CommandBuilder`] 用于链式绑定默认键位。
    ///
    /// **所有 catalog 都应当走这里**，而不是各自重复实现 "构造 CommandId →
    /// register → 构造 KeyBinding → keymap.bind" 这套样板。id / title /
    /// chord 都是命令模块里的源代码常量，运行时校验失败 = 程序员错误，
    /// 直接 panic 让启动崩，避免各处 `?` / `.expect` 的噪音。
    ///
    /// ```ignore
    /// registry.install(keymap, UNDO, "撤销", Box::new(run_undo))
    ///     .key("mod-z");
    /// registry.install(keymap, MOVE_SELECTION, "移动选区", Box::new(run_move))
    ///     .key_with("left", move_args(Previous, Grapheme, false))
    ///     .key_with("right", move_args(Next, Grapheme, false));
    /// ```
    pub fn install<'a>(
        &mut self,
        keymap: &'a mut Keymap,
        id: &'static str,
        title: &str,
        handler: CommandHandler,
    ) -> CommandBuilder<'a> {
        let command_id = CommandId::new(id).expect("命令 ID 必须非空");
        self.register(Command::new(command_id.clone(), title), handler)
            .expect("命令 ID 必须唯一");
        CommandBuilder { keymap, command_id }
    }

    pub fn command(&self, id: &CommandId) -> Option<&Command> {
        self.commands.get(id).map(|(command, _)| command)
    }

    pub fn handler(&self, id: &CommandId) -> Option<&CommandHandler> {
        self.commands.get(id).map(|(_, handler)| handler)
    }

    pub fn commands(&self) -> impl Iterator<Item = &Command> {
        self.commands.values().map(|(command, _)| command)
    }
}

/// `CommandRegistry::install` 的链式后续：绑定 0 ~ N 条默认键位。
///
/// 命令注册在 `install` 时已经完成；本 builder 只往 `Keymap` 写绑定。
/// drop 即结束 —— 不需要显式 finish。
pub struct CommandBuilder<'a> {
    keymap: &'a mut Keymap,
    command_id: CommandId,
}

impl<'a> CommandBuilder<'a> {
    /// 绑一条无参快捷键。chord 是源代码常量，空字符串会 panic。
    pub fn key(self, chord: &'static str) -> Self {
        self.key_with(chord, CommandArgs::new())
    }

    /// 绑一条带预设 args 的快捷键 —— 例如 `editor.move_selection` 不同
    /// 方向 / 粒度的变体都绑到同一条命令。
    pub fn key_with(self, chord: &'static str, args: CommandArgs) -> Self {
        let chord = KeyChord::new(chord).expect("快捷键必须非空");
        self.keymap.bind(KeyBinding {
            sequence: vec![chord],
            command: self.command_id.clone(),
            args,
            when: None,
        });
        self
    }

    /// 绑一条带 `when` 谓词的快捷键（mode / 模态触发）。
    #[allow(dead_code)]
    pub fn key_when(self, chord: &'static str, when: impl Into<String>) -> Self {
        let chord = KeyChord::new(chord).expect("快捷键必须非空");
        self.keymap.bind(KeyBinding {
            sequence: vec![chord],
            command: self.command_id.clone(),
            args: CommandArgs::new(),
            when: Some(when.into()),
        });
        self
    }
}

// ===== 执行上下文、队列与执行器 =====

/// 命令执行上下文。
///
/// 具体结构体而非 trait —— `zom-command` 本就依赖 `workspace` / `view`，
/// 直接命名它们，不做 trait 抽象体操。
///
/// **handler 接触不到的宿主资源**（GPUI Window、shell DockState 等）通过
/// `effects` emit 出去，宿主在派发结束后翻译。详见 [`crate::effects`]。
pub struct CommandContext<'a> {
    pub workspace: &'a mut Workspace,
    pub views: &'a mut ViewSet,
    /// 聚焦的输入框编辑目标。`Some` 时编辑命令作用于它而非主编辑区
    /// —— 由宿主（组合根）在派发前按 GPUI 焦点决定。
    pub focused_field: Option<EditTarget<'a>>,
    pub queue: &'a mut CommandQueue,
    pub effects: &'a mut EffectQueue,
}

/// 一次编辑命令作用的目标：文本缓冲 + 选区。
///
/// 把编辑命令与「buffer / selection 存放在哪」解耦 —— 主编辑区是 workspace
/// buffer + view selection，输入框是各自私有的 buffer + selection。
/// 编辑 handler 只认这个目标，不再直接穿 `workspace` / `views` 结构。
pub struct EditTarget<'a> {
    pub buffer: &'a mut zom_engine::Buffer,
    pub selection: &'a mut zom_engine::SelectionSet,
}

impl<'a> CommandContext<'a> {
    /// 解析当前编辑命令的作用目标：有聚焦输入框则作用于它，否则主编辑区
    /// 的活动视图。
    pub fn edit_target(&mut self) -> Result<EditTarget<'_>, CommandError> {
        if let Some(field) = &mut self.focused_field {
            return Ok(EditTarget {
                buffer: &mut *field.buffer,
                selection: &mut *field.selection,
            });
        }
        let buffer_id = self
            .views
            .active_view()
            .map(|view| view.buffer())
            .ok_or(CommandError::NoActiveView)?;
        let buffer = self
            .workspace
            .buffer_mut(buffer_id)
            .ok_or(CommandError::BufferNotFound(buffer_id))?
            .buffer_mut();
        let selection = self
            .views
            .active_view_mut()
            .ok_or(CommandError::NoActiveView)?
            .selection_mut();
        Ok(EditTarget { buffer, selection })
    }
}

/// 命令执行的产出，用于告知外壳后续动作（重绘、焦点变化等）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandOutcome {}

/// 命令队列。
///
/// handler 想触发子命令时入队，执行器排空 —— 不重入。
#[derive(Clone, Debug, Default)]
pub struct CommandQueue {
    pending: VecDeque<(CommandId, CommandArgs)>,
}

impl CommandQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn dispatch(&mut self, id: CommandId, args: CommandArgs) {
        self.pending.push_back((id, args));
    }

    pub fn pop(&mut self) -> Option<(CommandId, CommandArgs)> {
        self.pending.pop_front()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// 命令执行器：排空队列，对注册表 + 上下文执行。
#[derive(Default)]
pub struct CommandExecutor;

impl CommandExecutor {
    pub fn new() -> Self {
        Self
    }

    pub fn run(
        &self,
        registry: &CommandRegistry,
        context: &mut CommandContext<'_>,
    ) -> Result<(), CommandError> {
        while let Some((id, args)) = context.queue.pop() {
            let handler = registry
                .handler(&id)
                .ok_or_else(|| CommandError::UnknownCommand(id.clone()))?;
            handler(context, args)?;
        }

        Ok(())
    }
}

// ===== 键位模型 =====

/// 归一化后的单次组合键。
///
/// OS 事件 → 归一化 `KeyChord` 由 `zom-desktop` 完成；本 crate 只吃归一化结果。
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct KeyChord(String);

impl KeyChord {
    pub fn new(value: impl Into<String>) -> Result<Self, CommandError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(CommandError::InvalidKeyChord);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 多段按键序列（支持 leader key）。
pub type KeySequence = Vec<KeyChord>;

/// 一条键位绑定：序列 → 已成形的命令调用，可带 context 谓词。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyBinding {
    pub sequence: KeySequence,
    pub command: CommandId,
    pub args: CommandArgs,
    /// `when` 谓词；`None` 表示全局生效。
    pub when: Option<String>,
}

/// 键位表。内部维护前缀 trie，用于识别多段 key sequence。
#[derive(Clone, Debug, Default)]
pub struct Keymap {
    bindings: Vec<KeyBinding>,
    root: KeymapNode,
}

#[derive(Clone, Debug, Default)]
struct KeymapNode {
    children: BTreeMap<KeyChord, KeymapNode>,
    binding_indices: Vec<usize>,
}

impl Keymap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bind(&mut self, binding: KeyBinding) {
        let index = self.bindings.len();
        if !binding.sequence.is_empty() {
            let mut node = &mut self.root;
            for chord in &binding.sequence {
                node = node.children.entry(chord.clone()).or_default();
            }
            node.binding_indices.push(index);
        }
        self.bindings.push(binding);
    }

    pub fn bindings(&self) -> &[KeyBinding] {
        &self.bindings
    }

    /// 反查某条命令对应的快捷键序列（原始 `KeyChord` 序列）。
    ///
    /// UI 入口（Glyph、菜单、命令面板）通常只需要可显示的字符串 —— 用
    /// [`Self::format_shortcut_for`] 一步到位。需要做自定义处理（比如
    /// 折叠多绑定）时再用本方法拿原始序列。
    ///
    /// 同一命令绑了多条时返回第一条；带 `when` 谓词的绑定优先级低于全局绑定。
    pub fn shortcut_for(&self, command: &CommandId) -> Option<&KeySequence> {
        self.bindings
            .iter()
            .find(|binding| &binding.command == command && binding.when.is_none())
            .or_else(|| {
                self.bindings
                    .iter()
                    .find(|binding| &binding.command == command)
            })
            .map(|binding| &binding.sequence)
    }

    /// 反查 + 平台投影：给 UI 一个直接能展示的快捷键串。
    ///
    /// macOS 输出符号紧排（如 `⇧⌘Z`），其他平台输出文本拼接（如
    /// `Ctrl+Shift+Z`）。映射表见 [`crate::keymap_format`]。
    pub fn format_shortcut_for(&self, command: &CommandId) -> Option<String> {
        self.shortcut_for(command)
            .map(|sequence| keymap_format::format_sequence(sequence))
    }

    /// 按已输入的序列前缀解析。`contexts` 是当前活动的 `when` 上下文集合。
    pub fn resolve(&self, prefix: &[KeyChord], contexts: &[String]) -> KeymapResolution {
        let Some(node) = self.node_for_prefix(prefix) else {
            return KeymapResolution::NoMatch;
        };

        if let Some(binding) = node
            .binding_indices
            .iter()
            .rev()
            .filter_map(|index| self.bindings.get(*index))
            .find(|binding| binding_matches_context(binding, contexts))
        {
            return KeymapResolution::Matched {
                command: binding.command.clone(),
                args: binding.args.clone(),
            };
        }

        if node_has_context_match(node, &self.bindings, contexts) {
            KeymapResolution::Pending
        } else {
            KeymapResolution::NoMatch
        }
    }

    fn node_for_prefix(&self, prefix: &[KeyChord]) -> Option<&KeymapNode> {
        let mut node = &self.root;
        for chord in prefix {
            node = node.children.get(chord)?;
        }
        Some(node)
    }
}

/// 键位解析结果。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KeymapResolution {
    /// 是某个更长序列的前缀，等待后续按键。
    Pending,
    /// 命中一条绑定。
    Matched {
        command: CommandId,
        args: CommandArgs,
    },
    /// 无匹配。
    NoMatch,
}

// ===== 错误 =====

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandError {
    DuplicateCommand(CommandId),
    InvalidCommandId,
    InvalidKeyChord,
    UnknownCommand(CommandId),
    NoActiveView,
    BufferNotFound(BufferId),
    InvalidArgs(String),
    ExecutionFailed(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateCommand(id) => write!(f, "命令已注册：{id}"),
            Self::InvalidCommandId => f.write_str("命令 ID 不能为空"),
            Self::InvalidKeyChord => f.write_str("快捷键不能为空"),
            Self::UnknownCommand(id) => write!(f, "未知命令：{id}"),
            Self::NoActiveView => f.write_str("没有活动视图"),
            Self::BufferNotFound(id) => write!(f, "活动视图指向的 buffer 不存在：{}", id.as_u64()),
            Self::InvalidArgs(reason) => write!(f, "命令参数非法：{reason}"),
            Self::ExecutionFailed(reason) => write!(f, "命令执行失败：{reason}"),
        }
    }
}

impl std::error::Error for CommandError {}

// ===== 跨模块共用 helper（仅命令目录内部使用） =====

pub(crate) fn required_arg(args: &CommandArgs, key: &str) -> Result<String, CommandError> {
    args.get(key)
        .map(ToOwned::to_owned)
        .ok_or_else(|| CommandError::InvalidArgs(format!("缺少参数：{key}")))
}

pub(crate) fn reject_unknown_args(
    args: &CommandArgs,
    allowed: &[&str],
) -> Result<(), CommandError> {
    if let Some((key, _)) = args.fields().find(|(key, _)| !allowed.contains(key)) {
        return Err(CommandError::InvalidArgs(format!("未知参数：{key}")));
    }
    Ok(())
}

pub(crate) fn parse_optional_bool(value: Option<&str>) -> Result<bool, CommandError> {
    match value {
        None => Ok(false),
        Some("true" | "1" | "yes" | "on") => Ok(true),
        Some("false" | "0" | "no" | "off") => Ok(false),
        Some(other) => Err(CommandError::InvalidArgs(format!("布尔参数非法：{other}"))),
    }
}

pub(crate) fn command_execution_failed(error: EngineError) -> CommandError {
    CommandError::ExecutionFailed(error.to_string())
}

fn format_arg_keys(args: &CommandArgs) -> String {
    args.fields()
        .map(|(key, _)| key)
        .collect::<Vec<_>>()
        .join(", ")
}

fn binding_matches_context(binding: &KeyBinding, contexts: &[String]) -> bool {
    binding
        .when
        .as_ref()
        .is_none_or(|when| contexts.iter().any(|context| context == when))
}

fn node_has_context_match(node: &KeymapNode, bindings: &[KeyBinding], contexts: &[String]) -> bool {
    node.binding_indices
        .iter()
        .filter_map(|index| bindings.get(*index))
        .any(|binding| binding_matches_context(binding, contexts))
        || node
            .children
            .values()
            .any(|child| node_has_context_match(child, bindings, contexts))
}
