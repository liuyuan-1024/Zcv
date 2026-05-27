//! 编辑器视图层。

mod blink;
mod element;
mod input_host;
mod phases;
mod slot;

pub(crate) use blink::{CaretBlink, drive as drive_caret_blink};
pub(crate) use element::EditorElement;
pub(crate) use input_host::{EditorInputHook, EditorViewportSyncHook};
pub(crate) use slot::TextEditorSlot;
