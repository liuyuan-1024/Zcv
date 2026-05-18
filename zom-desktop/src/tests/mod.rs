//! 单元测试集中目录。
//!
//! `main.rs` 通过 `#[cfg(test)] mod tests;` 声明本目录。每个文件覆盖一个域，
//! 命名贴近被测对象：
//! - `app`：组合根派发管线（命令、IME、快捷键、HostEffect → WindowAction 翻译）
//!
//! 新增测试时优先往已有文件追加；测的是新域再开新文件，并在此处 `mod` 声明。
//!
//! 集中放置而非散落在各 `mod tests {}` 块里，避免重复 `use super::*;` /
//! 重复 import，也方便一眼看清整套用例覆盖了什么。

mod app;
