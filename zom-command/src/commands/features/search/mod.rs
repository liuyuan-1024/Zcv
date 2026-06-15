//! `search.*` 命令目录。
//!
//! 分两层：
//! - [`file`] —— 单文件（per-buffer）搜索 / 替换。唤起内联 bar。
//! - [`project`] —— 跨文件搜索（待实现）。当前只弹气泡占位。
//!
//! 两条命令链路完全独立：键位、HostEffect、宿主侧 handler 都分开走，不共用
//! 状态机。一处实现走偏不会影响另一处。

use crate::{CommandRegistry, Keymap};

pub mod file;
pub mod project;

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    file::install(registry, keymap);
    project::install(registry, keymap);
}
