//! M7A：编辑命令枚举。
//!
//! `Command` 是键盘、鼠标、菜单、命令面板、GPUI testbed 与未来宏录制的统一
//! 动作描述入口。M7A 只定义命令数据模型；命令上下文、来源标记、执行器与可回放
//! 序列化能力分别留给 M7B / M7C / M17。

use crate::{Selection, SelectionSet};

/// 统一编辑动作枚举。
///
/// 该类型只描述“用户或宿主想做什么”，不直接执行修改。后续 M7C 的
/// `CommandExecutor` 会负责把文本修改命令落到 `Transaction`，把移动命令落到
/// `SelectionSet`，并复用 M6C 的 composition 管线。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Command {
    // ---- 文本输入 ----
    /// 在当前选区处插入文本；非空选区会被替换。
    InsertText(String),
    /// 用同一段文本替换所有当前选区。
    ReplaceSelections(String),

    // ---- 删除 ----
    /// 删除当前非空选区；caret 本身不删除字符。
    DeleteSelection,
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
}
