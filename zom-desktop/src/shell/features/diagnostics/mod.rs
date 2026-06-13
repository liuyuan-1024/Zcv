//! 诊断功能。
//!
//! 当前只提供状态入口：功能图标归本模块所有。问题面板 UI 接入时在本目录补
//! `view.rs` / `state.rs` 等文件。

use gpui::AnyElement;

use crate::shell::shared::{CommandBinding, Glyph};

const INVOKER_ID: &str = "bottom-bar.diagnostics";

pub(crate) fn entry(count: u32, command: CommandBinding) -> AnyElement {
    Glyph::icon_text(
        INVOKER_ID,
        "icons/status/diagnostics.svg",
        count.to_string(),
    )
    .command(command)
    .render()
}
