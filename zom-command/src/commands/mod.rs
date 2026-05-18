//! 命令目录（catalog）。
//!
//! 每个域一个子模块，**同处**声明：
//! - `pub const ID_*` 命令 id 常量（单一真理源）
//! - `pub fn <name>() -> Invocation` 类型安全的调用构造器
//! - `install(registry, keymap, ...captures)` 一口气注册 handler + 默认键位
//!
//! 调用方不再到处 `CommandId::new("editor.foo")` / `CommandArgs::with("field", ...)`，
//! 也不必把 handler 和键位分散在两个文件里。
//!
//! 扩展域（panel.* / window.* / ai.* 等）走相同模式，但放在 zom-desktop 或
//! 业务 crate 里 —— 因为它们的 handler 需要捕获组合根侧的服务。

pub mod diagnostics;
pub mod editor;
pub mod panels;
pub mod settings;
pub mod window;
pub mod workspace;
