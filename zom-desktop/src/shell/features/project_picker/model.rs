use zom_command::{EditTarget, KeyContext};

use super::recent::RecentProject;
use crate::focus::{AppFocus, ProjectPickerFocus, SurfaceFocus};
use crate::shell::editor::{
    EditorSnapshot, EditorSnapshotRequest, ImeQueryTarget, ImeTarget, OwnedEditorTarget,
    TextTargetOwner, TextTargetQuery,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProjectPickerMode {
    Browse,
    CloneGit,
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectPickerState {
    pub(crate) query: EditorSnapshot,
    pub(crate) selected: usize,
    pub(crate) mode: ProjectPickerMode,
}

pub(crate) enum ProjectPickerActivation {
    None,
    Open(RecentProject),
    CloneGit(String),
}

pub(crate) struct ProjectPickerModel {
    query: OwnedEditorTarget,
    selected: usize,
    mode: ProjectPickerMode,
}

impl ProjectPickerModel {
    pub(crate) fn new() -> Self {
        Self {
            query: OwnedEditorTarget::new(),
            selected: 0,
            mode: ProjectPickerMode::Browse,
        }
    }

    pub(crate) fn reset(&mut self, mode: ProjectPickerMode) {
        self.query = OwnedEditorTarget::new();
        self.selected = 0;
        self.mode = mode;
    }

    pub(crate) fn state(&self) -> ProjectPickerState {
        ProjectPickerState {
            query: self.query.snapshot(EditorSnapshotRequest::single_line()),
            selected: self.selected,
            mode: self.mode,
        }
    }

    pub(crate) fn selected_project_id(&self, projects: &[RecentProject]) -> Option<String> {
        if self.mode != ProjectPickerMode::Browse {
            return None;
        }
        filtered_projects(projects, &self.query.text())
            .get(self.selected)
            .map(|project| project.id.clone())
    }

    pub(crate) fn move_selection(&mut self, delta: isize, projects: &[RecentProject]) {
        if self.mode != ProjectPickerMode::Browse {
            return;
        }
        let count = filtered_projects(projects, &self.query.text()).len();
        self.move_selection_by(delta, count);
    }

    pub(crate) fn clamp_selection(&mut self, projects: &[RecentProject]) {
        let count = filtered_projects(projects, &self.query.text()).len();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }

    pub(crate) fn reset_selection(&mut self) {
        self.selected = 0;
    }

    pub(crate) fn activation(&self, projects: &[RecentProject]) -> ProjectPickerActivation {
        if self.mode == ProjectPickerMode::CloneGit {
            let repo = self.query.text();
            let repo = repo.trim();
            if repo.is_empty() {
                return ProjectPickerActivation::None;
            }
            return ProjectPickerActivation::CloneGit(repo.to_string());
        }

        filtered_projects(projects, &self.query.text())
            .get(self.selected)
            .cloned()
            .map(ProjectPickerActivation::Open)
            .unwrap_or(ProjectPickerActivation::None)
    }

    fn move_selection_by(&mut self, delta: isize, count: usize) {
        if count == 0 {
            self.selected = 0;
            return;
        }
        // rem_euclid 保证负余数也落回 [0, count)，让 up / down 在头尾循环。
        let count_i = count as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(count_i) as usize;
    }
}

impl TextTargetQuery for ProjectPickerModel {
    fn accepts_focus(&self, focus: AppFocus) -> bool {
        matches!(
            focus,
            AppFocus::Surface(SurfaceFocus::ProjectPicker(ProjectPickerFocus::Query))
        )
    }

    fn snapshot(&self) -> EditorSnapshot {
        self.query.snapshot(EditorSnapshotRequest::single_line())
    }

    fn key_contexts(&self) -> Vec<KeyContext> {
        vec![
            KeyContext::project_picker(),
            KeyContext::text_edit(self.accepts_newline(), false),
            KeyContext::global(),
        ]
    }

    fn ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
        Some(self.query.as_ime_query_target())
    }
}

impl TextTargetOwner for ProjectPickerModel {
    fn ime_target(&mut self) -> Option<ImeTarget<'_>> {
        Some(self.query.as_ime_target())
    }

    fn edit_target(&mut self) -> Option<EditTarget<'_>> {
        Some(self.query.as_edit_target())
    }

    /// 查询文本一变就把候选 cursor 跳回第一项 —— 旧的下标可能超出新过滤结果。
    /// 由 [`super::EditorRouterMut::with_ime_target`] 在 IME 写入成功后调。
    fn after_text_changed(&mut self) {
        self.reset_selection();
    }
}

pub(crate) fn filtered_projects(projects: &[RecentProject], query: &str) -> Vec<RecentProject> {
    let terms = query
        .split_whitespace()
        .map(|term| term.to_lowercase())
        .collect::<Vec<_>>();
    if terms.is_empty() {
        return projects.to_vec();
    }

    projects
        .iter()
        .filter(|project| {
            let haystack = format!(
                "{} {} {}",
                project.name,
                project.identifier,
                project.path.display()
            )
            .to_lowercase();
            terms.iter().all(|term| haystack.contains(term))
        })
        .cloned()
        .collect()
}
