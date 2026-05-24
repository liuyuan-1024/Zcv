use zom_command::EditTarget;

use crate::app::RecentProject;
use crate::shell::editor::{Editor, EditorSnapshot, ImeQueryTarget, ImeTarget};

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
    query: Editor,
    selected: usize,
    mode: ProjectPickerMode,
    active: bool,
}

impl ProjectPickerModel {
    pub(crate) fn new() -> Self {
        Self {
            query: Editor::new(),
            selected: 0,
            mode: ProjectPickerMode::Browse,
            active: false,
        }
    }

    pub(crate) fn reset(&mut self, mode: ProjectPickerMode) {
        self.query = Editor::new();
        self.selected = 0;
        self.mode = mode;
        self.active = true;
    }

    pub(crate) fn deactivate(&mut self) {
        self.active = false;
    }

    pub(crate) fn active(&self) -> bool {
        self.active
    }

    pub(crate) fn state(&self) -> ProjectPickerState {
        ProjectPickerState {
            query: self.query.snapshot(),
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

    pub(crate) fn query_edit_target(&mut self) -> Option<EditTarget<'_>> {
        self.active.then(|| self.query.as_edit_target())
    }

    pub(crate) fn query_ime_target(&mut self) -> Option<ImeTarget<'_>> {
        self.active.then(|| self.query.as_ime_target())
    }

    pub(crate) fn query_ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
        self.active.then(|| self.query.as_ime_query_target())
    }

    fn move_selection_by(&mut self, delta: isize, count: usize) {
        if count == 0 {
            self.selected = 0;
            return;
        }
        let next = self.selected as isize + delta;
        self.selected = next.clamp(0, count as isize - 1) as usize;
    }
}

impl Default for ProjectPickerModel {
    fn default() -> Self {
        Self::new()
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
