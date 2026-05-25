//! Editor —— 唯一的可嵌入文本编辑单元。
//!
//! [`EditorEmbed`] 是业务组件使用的高层嵌入入口；底层绘制图元留在本模块
//! 内部。单行 / 多行只是它的一个 [`EditorKind`]，不存在第二套编辑器。
//! 焦点宿主、背景、空态等「外壳」仍属于嵌入处（工作台编辑区 / 文件树行）。
//!
//! 持有一份引擎 `Buffer` + 选区。编辑命令通过
//! [`zom_command::CommandContext::focused_field`] 路由到聚焦的 Editor，从而
//! 共享 `zom-command` 的全部编辑能力。IME 与编辑强绑定，故作为 Editor 能力。

mod blink;
mod core;
mod element;
mod embed;
mod highlight;
mod ime;
mod input;
mod main_editor;
mod owner;
mod profile;
mod router;
mod routing;
mod slot;

pub(crate) use blink::{CaretBlink, drive as drive_caret_blink};
pub(crate) use core::{Editor, EditorSnapshot, RevealHint};
pub(crate) use element::EditorKind;
pub(crate) use ime::{ImeQueryTarget, ImeTarget};
pub(crate) use input::EditorInput;
pub(crate) use main_editor::{MainEditorOwner, MainEditorOwnerRef};
pub(crate) use owner::{TextTargetOwner, TextTargetQuery};
pub(crate) use profile::TextInputProfile;
pub(crate) use router::{EditorRouter, EditorRouterMut};
pub(crate) use routing::TextTargetId;
pub(crate) use slot::TextEditorSlot;
