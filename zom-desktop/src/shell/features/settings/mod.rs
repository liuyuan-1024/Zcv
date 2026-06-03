//! 设置功能。
//!
//! 当前只提供入口：功能图标归本模块所有，命令标题归
//! `zom_command::commands::settings`。设置 UI 接入时在本目录补
//! `view.rs` / `state.rs` 等文件。

mod effects;
mod surface;
mod toml_editor;

pub(crate) use effects::try_apply_effect;
pub(crate) use toml_editor::SettingsTomlEditor;

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
