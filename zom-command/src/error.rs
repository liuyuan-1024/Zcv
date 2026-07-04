//! 命令系统错误类型。

use std::fmt;

use zom_workspace::BufferId;

use crate::{CommandId, KeyContext, KeySequence, keymap_format};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandError {
    DuplicateCommand(CommandId),
    DuplicateKeyBinding {
        sequence: KeySequence,
        context: KeyContext,
    },
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
            Self::DuplicateKeyBinding { sequence, context } => write!(
                f,
                "快捷键绑定上下文重叠（同一序列会被同一运行时上下文同时命中）：{} / {:?}",
                keymap_format::format_sequence(sequence),
                context
            ),
            Self::InvalidCommandId => f.write_str("命令 ID 不能为空"),
            Self::InvalidKeyChord => f.write_str("快捷键不能为空"),
            Self::UnknownCommand(id) => write!(f, "未知命令：{id}"),
            Self::NoActiveView => f.write_str("没有活动视图"),
            Self::BufferNotFound(id) => {
                write!(f, "活动视图指向的 buffer 不存在：{}", id.as_u64())
            }
            Self::InvalidArgs(reason) => write!(f, "命令参数非法：{reason}"),
            Self::ExecutionFailed(reason) => write!(f, "命令执行失败：{reason}"),
        }
    }
}

impl std::error::Error for CommandError {}
