//! 诊断功能。
//!
//! 当前只提供状态入口：功能图标归本模块所有。问题面板 UI 接入时在本目录补
//! `view.rs` / `state.rs` 等文件。

use gpui::AnyElement;

use crate::host_intent::CommandRequest;
use crate::shell::CommandPresentation;
use crate::shell::shared::Glyph;

const INVOKER_ID: &str = "bottom-bar.diagnostics";

pub(crate) fn entry(
    count: u32,
    command_request: CommandRequest,
    presentation: &CommandPresentation,
) -> AnyElement {
    Glyph::icon_text(
        INVOKER_ID,
        "icons/status/diagnostics.svg",
        count.to_string(),
        presentation.title.clone(),
    )
    .hint(presentation.hint.clone())
    .on_press(command_request)
    .render()
}
