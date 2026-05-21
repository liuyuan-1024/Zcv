//! 嵌入编辑器按键协议。
//!
//! 编辑器只消费会改变文本、选区、编辑历史或 IME composition 的按键。
//! 其余快捷键返回给父组件，由父组件解释业务语义或继续交给全局 keymap。

use zom_command::commands::editor as editor_commands;
use zom_command::{CommandId, HostEffect};

/// 嵌入编辑器的行模式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EditorLineMode {
    accepts_newline: bool,
}

impl EditorLineMode {
    /// 单行编辑器不消费 Enter / Return；它们通常是父组件的确认语义。
    pub(crate) fn single_line() -> Self {
        Self {
            accepts_newline: false,
        }
    }
}

/// 嵌入编辑器处理一次按键后的结果。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct EditorKeyOutcome {
    pub(crate) handled: bool,
    pub(crate) effects: Vec<HostEffect>,
}

impl EditorKeyOutcome {
    pub(crate) fn handled(effects: Vec<HostEffect>) -> Self {
        Self {
            handled: true,
            effects,
        }
    }

    pub(crate) fn bubble() -> Self {
        Self {
            handled: false,
            effects: Vec::new(),
        }
    }
}

/// 判定一条命令是否属于“编辑器应消费”的编辑行为。
///
/// `editor.select_tab` / `editor.close_tab` 虽然历史上放在 editor catalog，
/// 但它们操作的是主编辑区标签，不是文本编辑本身，因此必须冒泡给父组件。
pub(crate) fn is_editing_command(command: &CommandId, mode: EditorLineMode) -> bool {
    match command.as_str() {
        editor_commands::INSERT_TEXT
        | editor_commands::REPLACE_SELECTION
        | editor_commands::INDENT
        | editor_commands::OUTDENT
        | editor_commands::DELETE_BACKWARD
        | editor_commands::DELETE_FORWARD
        | editor_commands::SELECT_ALL
        | editor_commands::UNDO
        | editor_commands::REDO
        | editor_commands::MOVE_SELECTION
        | editor_commands::IME_COMMIT
        | editor_commands::IME_CANCEL => true,
        editor_commands::INSERT_NEWLINE => mode.accepts_newline,
        _ => false,
    }
}
