//! Editor —— 唯一的可嵌入文本编辑单元。
//!
//! [`EditorElement`] 是唯一的编辑器实现（一个 GPUI `Element`，可 `.child()`
//! 进任何容器）。单行 / 多行只是它的一个 [`EditorKind`]，不存在第二套编辑器。
//! 焦点宿主、背景、空态等「外壳」属于嵌入处（工作台编辑区 / 文件树行），
//! 不在本模块内。
//!
//! 持有一份引擎 `Buffer` + 选区。编辑命令通过
//! [`zom_command::CommandContext::focused_field`] 路由到聚焦的 Editor，从而
//! 共享 `zom-command` 的全部编辑能力。IME 与编辑强绑定，故作为 Editor 能力。

mod blink;
mod core;
mod element;
mod ime;
mod input;

pub(crate) use blink::{CARET_BLINK_INTERVAL, CaretBlink};
pub(crate) use core::{Editor, EditorSnapshot};
pub(crate) use element::{EditorElement, EditorKind};
pub(crate) use ime::{ImeQueryTarget, ImeTarget};
pub(crate) use input::EditorInput;
