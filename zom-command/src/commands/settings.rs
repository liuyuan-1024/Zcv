//! `settings.*` 命令目录。
//!
//! **尚未实现** —— 设置 UI 还没存在，所以 catalog 不暴露 install / handler /
//! typed builder。本模块只占住命令 id，让 UI 入口（top_bar 齿轮）可以引用
//! 常量而非裸字符串，保持"所有命令 id 字符串在 zom-command 仅出现一次"。
//!
//! 命令真正落地时：
//! 1. 在此加 `pub fn install(reg, km)` + handler；
//! 2. 在合适的位置绑默认键位（建议 `mod-,`）；
//! 3. emit `HostEffect::OpenSettings`（届时给 `HostEffect` 加变体）。

/// 打开设置面板。
pub const OPEN: &str = "settings.open";
