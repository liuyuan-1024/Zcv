//! 设置功能。
//!
//! 设置入口与可视化配置浮面。浮面里的 “打开 TOML” 会把真实 config.toml
//! 交给主编辑区打开。

mod effects;
mod surface;

pub(crate) use effects::try_apply_effect;

use gpui::{AnyElement, IntoElement};

use crate::host_intent::CommandRequest;
use crate::shell::CommandPresentation;
use crate::shell::shared::Glyph;
use crate::shell::surfaces::track_surface_anchor;

pub(crate) use surface::{
    SettingsIntent, SettingsIntentRequest, SettingsPanelState, SettingsRuntime, request,
};

pub(crate) const INVOKER_ID: &str = "top-bar.settings";

pub(crate) fn entry(
    active: bool,
    command_request: CommandRequest,
    presentation: &CommandPresentation,
) -> AnyElement {
    let glyph = Glyph::icon(
        INVOKER_ID,
        "icons/actions/settings.svg",
        presentation.title.clone(),
    )
    .hint(presentation.hint.clone())
    .active(active)
    .on_press(command_request)
    .render();

    track_surface_anchor(INVOKER_ID, glyph).into_any_element()
}
