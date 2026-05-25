//! 项目切换功能。

mod effects;
mod model;
mod recent;
mod surface;

pub(crate) use effects::try_apply_effect;

use std::rc::Rc;

use gpui::{AnyElement, IntoElement};
use zom_command::commands::project_picker as project_picker_commands;

use crate::shell::CommandTitleLookup;
use crate::shell::KeyRequest;
use crate::shell::ShortcutLookup;
use crate::shell::editor::TextEditorSlot;
use crate::shell::shared::Glyph;
use crate::shell::surfaces::track_surface_anchor;

pub(crate) use model::{
    ProjectPickerActivation, ProjectPickerMode, ProjectPickerModel, ProjectPickerState,
    filtered_projects,
};
pub(crate) use recent::{RecentProject, RecentProjects};
pub(crate) use surface::{ProjectPickerInitialMode, ProjectPickerRuntime, request};

/// 顶栏项目入口的稳定入口 id。功能 owns 它，承载它的 bar 只负责写入 element。
pub(crate) const INVOKER_ID: &str = "top-bar.workspace";

const COMMAND: &str = project_picker_commands::SHOW_PROJECTS_PICKER;

pub(crate) type ProjectListRequest = Rc<dyn Fn() -> Vec<RecentProject>>;
pub(crate) type ProjectPickerStateRequest = Rc<dyn Fn() -> ProjectPickerState>;

#[derive(Clone)]
pub(crate) struct ProjectPickerActions {
    pub(crate) projects: ProjectListRequest,
    pub(crate) state: ProjectPickerStateRequest,
    pub(crate) key_request: KeyRequest,
    pub(crate) slot: Rc<TextEditorSlot>,
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
