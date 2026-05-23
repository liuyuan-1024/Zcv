//! 命令目录（catalog）。
//!
//! 每个领域一个子模块，**同处**声明该领域的全部命令资产：
//! - `pub const <ID>` 命令 id 常量（单一真理源）
//! - typed args + 双向 `From` / `TryFrom<CommandArgs>` 转换
//! - `pub fn <name>() -> Invocation` 类型安全的调用构造器
//! - handler 与默认键位（`install` 一口气注册）
//! - 领域专属的键位上下文负载类型（如 [`editor::TextEditKeyContext`]）
//!

pub(crate) mod args;
pub mod diagnostics;
pub mod editor;
pub mod file_tree;
pub mod language_server;
pub mod panel;
pub mod settings;
pub mod surface;
pub mod window;
pub mod workspace;
