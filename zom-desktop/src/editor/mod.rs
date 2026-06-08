//! Editor —— 唯一的可嵌入文本编辑单元。
//!
//! [`TextEditorSlot`] 是业务组件使用的高层嵌入入口；底层绘制图元留在
//! [`view`] 层内部。调用方装配 slot 时通过 [`EditorKernel`] builder 自己
//! 拼出想要的能力（多行 / 单行、是否带 gutter、是否回写视口）。
//! 焦点宿主、背景、空态等「外壳」仍属于嵌入处（工作台编辑区 / 文件树行）。
//!
//! 自持小输入框由 [`zom_command::CommandContext::focused_field`] 路由到聚焦目标，从而共享 `zom-command` 的全部编辑能力。

pub(crate) mod highlight;
mod input;
mod kernel;
pub(crate) mod main_owner;
pub(crate) mod text;
mod view;

pub(crate) use kernel::EditorKernel;
pub(crate) use view::{
    CaretBlink, EditorViewportMeasurement, EditorViewportSyncHook, SettledViewportTop,
    TextEditorSlot, drive_caret_blink,
};
