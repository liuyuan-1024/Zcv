//! 命令注册表与默认键位 builder。

use std::collections::BTreeMap;

use crate::{
    Command, CommandArgs, CommandContext, CommandError, CommandId, CommandOutcome, KeyBinding,
    KeyBindingContext, KeyChord, Keymap,
};

/// 命令 handler：对 `CommandContext` 执行一次操作。
///
/// 内建编辑命令在 [`crate::commands::editor`] 注册；`ai.*`、未来 lsp/git 由
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

    /// 绑一条全局快捷键。
    pub fn key_with(self, chord: &'static str, args: CommandArgs) -> Self {
        self.key_with_in(chord, args, KeyBindingContext::global())
    }

    /// 绑一条指定上下文的快捷键。
    pub fn key_in(self, chord: &'static str, context: KeyBindingContext) -> Self {
        self.key_with_in(chord, CommandArgs::new(), context)
    }

    /// 绑一条指定上下文且带预设 args 的快捷键。
    pub fn key_with_in(
        self,
        chord: &'static str,
        args: CommandArgs,
        context: KeyBindingContext,
    ) -> Self {
        let chord = KeyChord::new(chord).expect("快捷键必须非空");
        self.keymap.bind(KeyBinding {
            sequence: vec![chord],
            command: self.command_id.clone(),
            args,
            context,
        });
        self
    }
}
