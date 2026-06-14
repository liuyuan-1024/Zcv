//! 设置功能。
//!
//! 设置入口与可视化配置浮面。浮面里的 “打开 TOML” 会把真实 config.toml
//! 交给主编辑区打开。

mod effects;
mod surface;

pub(crate) use effects::try_apply_effect;

use gpui::{AnyElement, IntoElement};

use crate::shell::shared::{CommandBinding, Glyph};
use crate::theme::color;
use crate::shell::surfaces::track_surface_anchor;

pub(crate) use surface::{
    SettingsIntent, SettingsIntentRequest, SettingsPanelState, SettingsRuntime, request,
};

pub(crate) const INVOKER_ID: &str = "top-bar.settings";

pub(crate) fn entry(active: bool, command: CommandBinding) -> AnyElement {
    let entry_color = if active {
        color::glyph_active()
    } else {
        color::glyph_default()
    };
    let glyph = Glyph::icon(INVOKER_ID, "icons/actions/settings.svg")
        .color(entry_color)
        .command(command)
        .render();

    track_surface_anchor(INVOKER_ID, glyph).into_any_element()
}
