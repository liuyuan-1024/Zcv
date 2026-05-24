//! 跨模块测试入口。
//!
//! `main.rs` 通过 `#[cfg(test)] mod tests;` 声明本目录。每个文件覆盖一个域，
//! 命名贴近被测对象：
//! - `app`：组合根派发管线（命令、IME、快捷键、HostEffect 输出）
//! - `workbench`：窗口布局状态控制器
//!
//! 新增测试默认遵循 workspace 测试策略：模块内部不变量和私有 helper
//! 放到被测源码旁边，只有跨多个 desktop 模块的组合根行为才继续放在这里。
//! 测的是新跨模块域再开新文件，并在此处 `mod` 声明。

mod app;
mod workbench;
