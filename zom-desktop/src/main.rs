//! zom-desktop —— GPUI 外壳 + 组合根
//!
//! `shell` 负责视觉与平台；`app` 负责组合根。本入口只把两者拼起来。
//!
//! 顶层模块布局：
//! - `app/` —— 组合根 App + 它的私有协作者（command_runtime / config_store / config_applier / text_target_runtime / pumps）。
//! 对外只暴露 `App` 一个名字。
//! - `shell/` —— GPUI 外壳与 features。**不能** `use crate::app::*`（白名单仅 `crate::app::App` 在 boot 时构造一次）。
//! - `editor/` —— 可嵌入文本编辑器组件（kernel / input / view / text / highlight）。被 shell features 与 workbench 嵌入复用。
//! - `config` / `focus` / `editor_state` / `workspace_session` / `ports` / `text_target` —— 顶层共享词汇表，
//! app 与 shell 共用：数据 schema、语义焦点、编辑区标签摘要、workspace 薄壳、反向接入端口、文本目标路由协议。
//! - `theme` / `clipboard` —— 顶层共享基础设施：主题色 token / 字号 / 圆角，以及 GPUI 剪贴板适配层；shell 与可嵌入编辑器都依赖。

// Windows: GUI 应用不弹出控制台窗口。
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod app;
mod clipboard;
mod config;
mod dispatch;
mod editor;
mod editor_state;
mod file_watcher;
mod focus;
mod git_service;
mod host_intent;
mod lsp_host;
mod ports;
mod shell;
mod text_target;
mod theme;
mod ui_id;
mod workspace_session;

fn main() {
    shell::run(app::App::new_persistent());
}
