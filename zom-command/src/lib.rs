//! zom-command —— 命令派发脊柱 + 键位模型
//!
//! 「所有操作均是命令」。键盘、命令面板、AI、菜单都把意图收敛成
//! `(CommandId, CommandArgs)`，经唯一派发路径进入执行器（参数模型方案 A）。
//!
//! 依赖 `zom-workspace` / `zom-view` / `zom-engine`（它要编辑的东西），
//! **不依赖** `zom-ai` 等扩展域 —— 扩展域的命令由 `zom-desktop` 组合根
//! 注册（handler 闭包捕获扩展服务）。
//!
//! ## 模块划分
//!
//! 下列模块均私有（`mod`），其类型在 crate 根经 `pub use` 重导出 ——
//! 对外只有 `zom_command::CommandId` 一条路径，不暴露内部模块名。
//! - `core`：命令基础类型 —— `CommandId / CommandArgs / NoArgs / Command /
//!   Invocation`。
//! - `registry`：开放注册表 `CommandRegistry / CommandHandler` 与链式
//!   `CommandBuilder`。
//! - `executor`：执行上下文与队列 —— `CommandContext / EditTarget /
//!   CommandQueue / CommandExecutor`。
//! - `keymap`：键位模型 —— `KeyChord / KeyBinding / Keymap / KeyContext`。
//! - `error`：统一错误 `CommandError`。
//!
//! 以下模块对外公开：
//! - [`effects`] / [`keymap_format`]：宿主副作用队列、快捷键平台投影。
//! - [`commands`]：按域分组的"命令目录"。每个域一个子模块，**同处**声明：
//!   常量 id、typed args、typed builders、handler、默认键位 —— 域专属的键位
//!   上下文类型（如 `TextEditKeyContext`、`FileTreeKeyContext`）也内聚在对应
//!   域模块里。本文件只做模块编排与重导出，不直接持有任何具体定义。

pub mod commands;
pub mod effects;
pub mod keymap_format;

mod clipboard;
mod core;
mod error;
mod executor;
mod keymap;
mod registry;

pub use clipboard::{ClipboardPort, MockClipboard};
pub use commands::editor::{CompositionBinding, TextEditBindingContext, TextEditKeyContext};
pub use commands::file_tree::{FileTreeBindingContext, FileTreeKeyContext, FileTreeKeyMode};
pub use commands::project_picker::{ProjectPickerBindingContext, ProjectPickerKeyContext};
pub use core::{Command, CommandArgs, CommandId, Invocation, NoArgs};
pub use effects::{EffectQueue, HostEffect, SearchOption};
pub use error::CommandError;
pub use executor::{CommandContext, CommandExecutor, CommandOutcome, CommandQueue, EditTarget};
pub use keymap::{
    KeyBinding, KeyBindingContext, KeyChord, KeyContext, KeySequence, Keymap, KeymapResolution,
};
pub use registry::{CommandBuilder, CommandHandler, CommandRegistry};

pub(crate) use commands::args::{
    command_execution_failed, format_arg_keys, parse_optional_bool, reject_unknown_args,
    required_arg,
};
pub(crate) use executor::active_view_buffer_id;
