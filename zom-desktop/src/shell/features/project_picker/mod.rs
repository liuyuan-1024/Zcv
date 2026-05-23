//! 项目切换功能。

mod surface;

use gpui::{AnyElement, IntoElement};
use zom_command::commands::workspace as workspace_commands;

use crate::shell::ShortcutLookup;
use crate::shell::shared::Glyph;
use crate::shell::surfaces::track_surface_anchor;

pub(crate) use surface::request;

/// 功能显示名（顶栏 tooltip 与 surface 标题共用）—— 名字归功能自己持有。
pub(crate) const FEATURE_TITLE: &str = "切换项目";
/// 顶栏项目入口的稳定入口 id。功能 owns 它，承载它的 bar 只负责写入 element。
pub(crate) const INVOKER_ID: &str = "top-bar.workspace";

const COMMAND: &str = workspace_commands::SHOW_PROJECTS_PICKER;

pub(crate) fn entry(project_title: &str, active: bool, shortcuts: &ShortcutLookup) -> AnyElement {
    let glyph = Glyph::text(INVOKER_ID, project_title, FEATURE_TITLE)
        .hint(shortcuts(COMMAND))
        .active(active)
        .render();

    track_surface_anchor(INVOKER_ID, glyph).into_any_element()
}
