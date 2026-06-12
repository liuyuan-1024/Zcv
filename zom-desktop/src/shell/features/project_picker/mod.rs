//! 项目切换功能。

mod effects;
mod model;
mod recent;
mod surface;

pub(crate) use effects::try_apply_effect;

use std::rc::Rc;

use gpui::{AnyElement, IntoElement};

use crate::editor::TextEditorSlot;
use crate::host_intent::CommandRequest;
use crate::shell::CommandPresentation;
use crate::shell::CommandTitleLookup;
use crate::shell::ShortcutLookup;
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

pub(crate) type ProjectListRequest = Rc<dyn Fn() -> Vec<RecentProject>>;
pub(crate) type ProjectPickerStateRequest = Rc<dyn Fn() -> ProjectPickerState>;
pub(crate) type ProjectPickerIntentRequest =
    Rc<dyn Fn(ProjectPickerIntent, &mut gpui::Window, &mut gpui::App)>;

#[derive(Clone)]
pub(crate) enum ProjectPickerIntent {
    RemoveRecentProject { id: String },
}

#[derive(Clone)]
pub(crate) struct ProjectPickerActions {
    pub(crate) projects: ProjectListRequest,
    pub(crate) state: ProjectPickerStateRequest,
    pub(crate) slot: Rc<TextEditorSlot>,
    pub(crate) intent_request: ProjectPickerIntentRequest,
    pub(crate) remove_recent_presentation: CommandPresentation,
    pub(crate) shortcut_lookup: ShortcutLookup,
    pub(crate) command_title_lookup: CommandTitleLookup,
}

pub(crate) fn entry(
    project_title: &str,
    active: bool,
    command_request: CommandRequest,
    presentation: &CommandPresentation,
) -> AnyElement {
    let glyph = Glyph::text(INVOKER_ID, project_title, presentation.title.clone())
        .hint(presentation.hint.clone())
        .active(active)
        .on_press(command_request)
        .render();

    track_surface_anchor(INVOKER_ID, glyph).into_any_element()
}
