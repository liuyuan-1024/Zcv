//! 命令派发脊柱与键位模型。
//!
//! 键盘、命令面板、AI、菜单等离散动作把意图收敛成 `(CommandId, CommandArgs)`，经唯一命令派发路径进入执行器。
//! 鼠标拖拽、滚轮、resize 等连续设备交互由宿主的 interaction 管线处理，可复用同一批底层编辑状态能力，但不进入 command catalog。
//!
//! 依赖 `zom-workspace` / `zom-engine`（它要编辑的东西），
//! **不依赖** `zom-ai` 等扩展域 —— 扩展域的命令由 `zom-desktop` 组合根注册（handler 闭包捕获扩展服务）。
//!
//! ## 模块划分
//!
//! 下列模块均私有（`mod`），其类型在 crate 根经 `pub use` 重导出 —— 对外只有 `zom_command::CommandId` 一条路径，不暴露内部模块名。
//! - `core`：命令基础类型 —— `CommandId / CommandArgs / NoArgs / Command / Invocation`。
//! - `registry`：开放注册表 `CommandRegistry / CommandHandler` 与链式 `CommandBuilder`。
//! - `executor`：执行上下文与队列 —— `CommandContext / EditTarget / CommandQueue` 与排空入口 `run`。
//! - `keymap`：键位模型 —— `KeyChord / KeyBinding / Keymap / KeyContext`。
//! - `error`：统一错误 `CommandError`。
//!
//! 以下模块对外公开：
//! - [`effects`] / [`keymap_format`]：宿主副作用队列、快捷键平台投影。
//! - [`commands`]：按域分组的「命令目录」。每个域一个子模块，**同处**声明：常量 id、typed args、typed builders、handler、默认键位 —— 域专属的键位上下文类型（如 `TextEditKeyContext`、`FileTreeKeyContext`）也内聚在对应域模块里。
//!   本文件只做模块编排与重导出，不直接持有任何具体定义。

pub mod commands;
pub mod effects;
pub mod keymap_format;

mod clipboard;
mod core;
mod dismiss;
mod error;
mod executor;
mod keymap;
mod registry;

pub use clipboard::{ClipboardPort, NoopClipboard};
pub use commands::editor::{CompositionBinding, TextEditBindingContext, TextEditKeyContext};
pub use commands::file_tree::{FileTreeBindingContext, FileTreeKeyContext, FileTreeKeyMode};
pub use commands::project_picker::{ProjectPickerBindingContext, ProjectPickerKeyContext};
pub use core::{Command, CommandArgs, CommandCatalogItem, CommandId, Invocation, NoArgs};
pub use dismiss::{DismissScope, DismissStacks, DismissTokenId};
pub use effects::{
    BubbleKind, BubbleRequest, EffectQueue, HostEffect, PanelKind, SearchOption,
    SettingsChangeRequest,
};
pub use error::CommandError;
pub use executor::{
    CommandContext, CommandOutcome, CommandQueue, EditTarget, reconcile_after_input_mutation, run,
};
pub use keymap::{
    KeyBinding, KeyBindingContext, KeyChord, KeyContext, KeySequence, Keymap, KeymapResolution,
};
pub use registry::{CommandBuilder, CommandHandler, CommandRegistry};

pub(crate) use commands::args::{
    command_execution_failed, format_arg_keys, parse_optional_bool, reject_unknown_args,
    required_arg,
};
pub(crate) use executor::active_view_buffer_id;
