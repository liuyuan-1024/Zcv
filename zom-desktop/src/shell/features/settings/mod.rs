//! 设置功能。
//!
//! 设置入口与可视化配置浮面。浮面里的 “打开 TOML” 会把真实 config.toml
//! 交给主编辑区打开。

mod effects;
mod surface;

pub(crate) use effects::try_apply_effect;

use gpui::{AnyElement, IntoElement};
use zom_command::commands::settings;

use crate::shell::shared::Glyph;
use crate::shell::surfaces::track_surface_anchor;
use crate::shell::{CommandTitleLookup, ShortcutLookup};

pub(crate) use surface::{
    SettingsAction, SettingsActionRequest, SettingsPanelState, SettingsRuntime, request,
};

pub(crate) const INVOKER_ID: &str = "top-bar.settings";
const COMMAND: &str = settings::OPEN;

pub(crate) fn entry(
    active: bool,
    shortcuts: &ShortcutLookup,
    titles: &CommandTitleLookup,
) -> AnyElement {
    let title = titles(COMMAND).unwrap_or_else(|| COMMAND.to_string());
    let glyph = Glyph::icon(INVOKER_ID, "icons/actions/settings.svg", title)
        .hint(shortcuts(COMMAND))
        .active(active)
        .render();

    track_surface_anchor(INVOKER_ID, glyph).into_any_element()
}
