//! 语言服务器状态功能。

mod surface;

use gpui::{AnyElement, IntoElement};
use zom_command::commands::language_servers;

use crate::shell::shared::Glyph;
use crate::shell::surfaces::track_surface_anchor;
use crate::shell::{CommandTitleLookup, ShortcutLookup};

pub(crate) use surface::{LanguageServersRuntime, request};

/// 该功能在底栏的图标 —— 视觉身份归功能自己持有，承载它的 bar 不重新描述。
pub(crate) const BAR_ICON: &str = "icons/bottom_bar/language_server.svg";
/// 底栏语言服务器入口的稳定入口 id。
pub(crate) const INVOKER_ID: &str = "bottom-bar.language_server";

const COMMAND: &str = language_servers::OPEN_STATUS;

pub(crate) fn entry(
    connected: bool,
    active: bool,
    shortcuts: &ShortcutLookup,
    titles: &CommandTitleLookup,
) -> AnyElement {
    let title = titles(COMMAND).unwrap_or_else(|| COMMAND.to_string());
    let glyph = Glyph::icon(INVOKER_ID, BAR_ICON, title)
        .hint(shortcuts(COMMAND))
        .active(connected || active)
        .render();

    track_surface_anchor(INVOKER_ID, glyph).into_any_element()
}
