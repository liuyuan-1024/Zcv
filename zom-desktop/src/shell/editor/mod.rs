//! Editor —— 可嵌入的文本编辑单元。
//!
//! 持有一份引擎 `Buffer` + 选区。主编辑区与各 feature 的输入框都可以是
//! Editor 实例；编辑命令通过 [`zom_command::CommandContext::focused_field`]
//! 路由到聚焦的 Editor，从而共享 `zom-command` 的全部编辑能力（插入 /
//! 删除 / undo / redo / 选择 / 移动…），无需各自重写。
//!
//! 本模块只持「编辑行为」状态，不含文件概念（路径 / dirty / 标签）——那些
//! 属于 `zom-workspace`。IME 与编辑强绑定，故放在这里作为 Editor 能力。

mod core;
mod grid;
mod ime;
mod inline;
mod input;
mod key;

pub(crate) use core::{Editor, EditorSnapshot};
pub(crate) use grid::render_grid;
pub(crate) use ime::{ImeQueryTarget, ImeTarget};
pub(crate) use inline::render_inline;
pub(crate) use input::EditorInput;
pub(crate) use key::{EditorKeyOutcome, EditorLineMode, is_editing_command};
