//! 项目切换功能。

mod surface;

use std::rc::Rc;

use gpui::{AnyElement, IntoElement};
use zom_command::commands::project_picker as project_picker_commands;

use crate::app::RecentProject;
use crate::shell::CommandTitleLookup;
use crate::shell::KeyRequest;
use crate::shell::ShortcutLookup;
use crate::shell::shared::Glyph;
use crate::shell::surfaces::track_surface_anchor;

pub(crate) use surface::{
    ProjectPickerActivation, ProjectPickerInitialMode, ProjectPickerRuntime, request,
};

/// 顶栏项目入口的稳定入口 id。功能 owns 它，承载它的 bar 只负责写入 element。
pub(crate) const INVOKER_ID: &str = "top-bar.workspace";

const COMMAND: &str = project_picker_commands::SHOW_PROJECTS_PICKER;

pub(crate) type ProjectListRequest = Rc<dyn Fn() -> Vec<RecentProject>>;
pub(crate) type QueryTextRequest = Rc<dyn Fn(String, &mut gpui::Window, &mut gpui::App)>;

#[derive(Clone)]
pub(crate) struct ProjectPickerActions {
    pub(crate) projects: ProjectListRequest,
    pub(crate) key_request: KeyRequest,
    pub(crate) query_text_request: QueryTextRequest,
    pub(crate) shortcut_lookup: ShortcutLookup,
    pub(crate) command_title_lookup: CommandTitleLookup,
}

pub(crate) fn entry(
    project_title: &str,
    active: bool,
    shortcuts: &ShortcutLookup,
    titles: &CommandTitleLookup,
) -> AnyElement {
    let title = titles(COMMAND).unwrap_or_else(|| COMMAND.to_string());
    let glyph = Glyph::text(INVOKER_ID, project_title, title)
        .hint(shortcuts(COMMAND))
        .active(active)
        .render();

    track_surface_anchor(INVOKER_ID, glyph).into_any_element()
}
