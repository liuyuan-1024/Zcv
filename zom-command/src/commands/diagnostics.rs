//! `diagnostics.*` 命令目录。
//!
//! **尚未实现** —— 诊断面板 / LSP 状态面板还没存在。本模块只占住命令 id，
//! 让 bottom_bar 状态 glyph 可以引用常量而非裸字符串，保持"所有命令 id
//! 字符串在 zom-command 仅出现一次"。
//!
//! 命令真正落地时：在此加 `install` + handler + 默认键位，并视需要给
//! `HostEffect` 加变体。

/// 打开"问题"面板查看诊断列表。
pub const SHOW_PROBLEMS: &str = "diagnostics.show_problems";

/// 打开 LSP 服务连接状态面板。
pub const OPEN_LSP_STATUS: &str = "diagnostics.open_lsp_status";
