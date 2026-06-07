//! `search.*` 命令目录。
//!
//! 分两层：
//! - [`file`] —— 单文件（per-buffer）搜索 / 替换。`mod-f` 唤起内联 bar。
//! - [`project`] —— 跨文件搜索（待实现）。`mod-shift-f` 当前只弹气泡占位。
//!
//! 两条命令链路完全独立：键位、HostEffect、宿主侧 handler 都分开走，不共用
//! 状态机。一处实现走偏不会影响另一处。

use crate::{CommandRegistry, Keymap};

pub mod file;
pub mod project;

// 兼容旧引用路径：`search::ACTIVATE` / `search::activate()` 等仍然可用。
// 子模块拆分对外是无感的，下游 import `search::*` 不需要改。
pub use file::{
    ACTIVATE, CONFIRM_MATCH, DISMISS, FIND_NEXT, FIND_PREVIOUS, FOCUS_NEXT_FIELD,
    FOCUS_PREVIOUS_FIELD, REPLACE_ALL, REPLACE_NEXT, TOGGLE_CASE_SENSITIVE, TOGGLE_REGEX,
    TOGGLE_WHOLE_WORD, activate, confirm_match, dismiss, find_next, find_previous,
    focus_next_field, focus_previous_field, replace_all, replace_next, toggle_case_sensitive,
    toggle_regex, toggle_whole_word,
};
pub use project::{PROJECT_ACTIVATE, project_activate};

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    file::install(registry, keymap);
    project::install(registry, keymap);
}
