//! 语言服务器状态功能。

mod effects;
mod surface;

pub(crate) use effects::try_apply_effect;

use gpui::{AnyElement, IntoElement};

use crate::host_intent::CommandRequest;
use crate::shell::CommandPresentation;
use crate::shell::shared::Glyph;
use crate::shell::surfaces::track_surface_anchor;

pub(crate) use surface::{LanguageServersRuntime, request};

/// 底栏语言服务器入口的稳定入口 id。
pub(crate) const INVOKER_ID: &str = "bottom-bar.language_server";

pub(crate) fn entry(
    connected: bool,
    active: bool,
    command_request: CommandRequest,
    presentation: &CommandPresentation,
) -> AnyElement {
    let glyph = Glyph::icon(
        INVOKER_ID,
        "icons/status/language_server.svg",
        presentation.title.clone(),
    )
    .hint(presentation.hint.clone())
    .active(connected || active)
    .on_press(command_request)
    .render();

    track_surface_anchor(INVOKER_ID, glyph).into_any_element()
}
