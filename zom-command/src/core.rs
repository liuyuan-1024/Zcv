//! 命令基础类型：id、参数、元数据与 Invocation。

use std::collections::BTreeMap;
use std::fmt;

use crate::{CommandError, format_arg_keys};

/// 一次命令调用所需的两个组件，等价于"未提交的派发请求"。
///
/// 各 catalog 的 typed builders（如 `editor::insert_text(...)`）都返回此别名 ——
/// 调用方拿到后只需 `app.dispatch(invocation)`，无需再手拼 id 字符串或 args。
pub type Invocation = (CommandId, CommandArgs);

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
/// 派发边界统一为此类型，每条命令在自己的目录模块里通过
/// `TryFrom<CommandArgs>` 解析成强类型参数，构造侧则通过 `From<TypedArgs>`
/// 反向生成 —— 字段名只在命令模块出现一次。
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

/// 命令的元数据。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Command {
    pub id: CommandId,
    pub title: String,
    pub description: Option<String>,
    pub visible_in_shortcuts: bool,
}

impl Command {
    pub fn new(id: CommandId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
            description: None,
            visible_in_shortcuts: false,
        }
    }

    /// 设置快捷键面板列表行展示的一句话解释，并让命令进入快捷键面板。
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self.visible_in_shortcuts = true;
        self
    }

    pub fn hidden_from_shortcuts(mut self) -> Self {
        self.visible_in_shortcuts = false;
        self
    }
}

/// 命令系统暴露给宿主的只读命令元数据。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandCatalogItem {
    pub command_id: String,
    pub title: String,
    pub description: Option<String>,
    pub visible_in_shortcuts: bool,
}

impl From<&Command> for CommandCatalogItem {
    fn from(command: &Command) -> Self {
        Self {
            command_id: command.id.to_string(),
            title: command.title.clone(),
            description: command.description.clone(),
            visible_in_shortcuts: command.visible_in_shortcuts,
        }
    }
}
