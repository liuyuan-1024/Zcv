//! zom-desktop —— GPUI 外壳 + 组合根
//!
//! `shell` 负责视觉与平台；`app` 负责组合根。本入口只把两者拼起来。
//!
//! 顶层模块布局：
//! - `app/` —— 组合根 App + 它的私有协作者（command_runtime / config_store / config_applier / text_target_runtime / pumps）。
//! 对外只暴露 `App` 一个名字。
//! - `shell/` —— GPUI 外壳与 features。**不能** `use crate::app::*`（白名单仅 `crate::app::App` 在 boot 时构造一次）。
//! - `config` / `focus` / `editor_state` / `workspace_session` / `ports` / `text_target` —— 顶层共享词汇表，
//! app 与 shell 共用：数据 schema、语义焦点、编辑区标签摘要、workspace 薄壳、反向接入端口、文本目标路由协议。

mod app;
mod config;
mod dispatch;
mod editor_state;
mod focus;
mod ports;
mod shell;
mod text_target;
mod workspace_session;

fn main() {
    shell::run(app::App::new_persistent());
}
