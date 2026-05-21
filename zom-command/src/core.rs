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
}

impl Command {
    pub fn new(id: CommandId, title: impl Into<String>) -> Self {
        Self {
            id,
            title: title.into(),
        }
    }
}
