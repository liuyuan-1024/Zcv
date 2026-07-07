//! 编辑器视图层。

mod blink;
mod element;
mod fluid_selection;
mod gutter;
mod input_host;
mod phases;
mod slot;

pub(crate) use blink::{CaretBlink, drive as drive_caret_blink};
pub(crate) use element::EditorElement;
pub(crate) use input_host::{
    EditorInputHook, EditorViewportMeasurement, EditorViewportSyncHook, SettledViewportTop,
};
pub(crate) use slot::TextEditorSlot;
