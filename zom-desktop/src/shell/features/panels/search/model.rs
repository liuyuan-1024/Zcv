use zom_command::{EditTarget, SearchOption, SearchScope};
use zom_engine::TextRange;
use zom_workspace::BufferId;

use crate::shell::editor::{
    Editor, EditorSnapshot, ImeQueryTarget, ImeTarget, TextInputProfile, TextTargetId,
    TextTargetOwner, TextTargetQuery,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct SearchState {
    pub(crate) query: EditorSnapshot,
    pub(crate) replacement: EditorSnapshot,
    pub(crate) scope: SearchScope,
    pub(crate) options: SearchOptions,
    pub(crate) results: Vec<SearchResultItem>,
    pub(crate) active_result: Option<usize>,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SearchOptions {
    pub(crate) case_sensitive: bool,
    pub(crate) whole_word: bool,
    pub(crate) regex: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SearchResultItem {
    pub(crate) buffer_id: BufferId,
    pub(crate) title: String,
    pub(crate) range: TextRange,
    pub(crate) buffer_ordinal: usize,
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) preview: String,
}

#[derive(Default)]
pub(crate) struct SearchModel {
    query: Editor,
    replacement: Editor,
    scope: SearchScope,
    options: SearchOptions,
    results: Vec<SearchResultItem>,
    active_result: Option<usize>,
    error: Option<String>,
    active: Option<TextTargetId>,
    /// 上一次 `search_refresh` 看到的 query 文本，给「文本变了再触发」用。
    last_synced_query: String,
}

impl SearchModel {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn state(&self) -> SearchState {
        SearchState {
            query: self.query.snapshot(),
            replacement: self.replacement.snapshot(),
            scope: self.scope,
            options: self.options,
            results: self.results.clone(),
            active_result: self.active_result,
            error: self.error.clone(),
        }
    }

    pub(crate) fn query_text(&self) -> String {
        self.query.text()
    }

    pub(crate) fn replacement_text(&self) -> String {
        self.replacement.text()
    }

    pub(crate) fn scope(&self) -> SearchScope {
        self.scope
    }

    pub(crate) fn options(&self) -> SearchOptions {
        self.options
    }

    pub(crate) fn results(&self) -> &[SearchResultItem] {
        &self.results
    }

    pub(crate) fn active_result(&self) -> Option<usize> {
        self.active_result
    }

    pub(crate) fn set_results(
        &mut self,
        results: Vec<SearchResultItem>,
        active_result: Option<usize>,
        error: Option<String>,
    ) {
        self.results = results;
        self.active_result = active_result.filter(|index| *index < self.results.len());
        self.error = error;
        self.last_synced_query = self.query.text();
    }

    pub(crate) fn clear_results(&mut self) {
        self.results.clear();
        self.active_result = None;
        self.error = None;
        self.last_synced_query = self.query.text();
    }

    /// 调用方在每次可能改动 query 文本之后调用；返回 true 表示文本与上一次
    /// 落地的搜索结果不一致，需要重新搜索。
    pub(crate) fn query_changed_since_last_sync(&self) -> bool {
        self.query.text() != self.last_synced_query
    }

    pub(crate) fn set_scope(&mut self, scope: SearchScope) {
        self.scope = scope;
    }

    pub(crate) fn toggle_option(&mut self, option: SearchOption) {
        match option {
            SearchOption::CaseSensitive => {
                self.options.case_sensitive = !self.options.case_sensitive
            }
            SearchOption::WholeWord => self.options.whole_word = !self.options.whole_word,
            SearchOption::Regex => self.options.regex = !self.options.regex,
        }
    }

    pub(crate) fn activate(&mut self, target: TextTargetId) {
        if is_search_target(target) {
            self.active = Some(target);
        }
    }

    pub(crate) fn deactivate(&mut self, target: TextTargetId) {
        if self.active == Some(target) {
            self.active = None;
        }
    }

    pub(crate) fn active(&self) -> bool {
        self.active.is_some()
    }

    pub(crate) fn edit_target(&mut self) -> Option<EditTarget<'_>> {
        match self.active {
            Some(TextTargetId::SearchQuery) => Some(self.query.as_edit_target()),
            Some(TextTargetId::SearchReplacement) => Some(self.replacement.as_edit_target()),
            _ => None,
        }
    }

    pub(crate) fn query_owner(&self) -> SearchFieldQuery<'_> {
        SearchFieldQuery {
            model: self,
            target: TextTargetId::SearchQuery,
        }
    }

    pub(crate) fn replacement_owner(&self) -> SearchFieldQuery<'_> {
        SearchFieldQuery {
            model: self,
            target: TextTargetId::SearchReplacement,
        }
    }

    pub(crate) fn active_owner(&mut self) -> SearchActiveOwner<'_> {
        SearchActiveOwner { model: self }
    }
}

pub(crate) struct SearchFieldQuery<'a> {
    model: &'a SearchModel,
    target: TextTargetId,
}

pub(crate) struct SearchActiveOwner<'a> {
    model: &'a mut SearchModel,
}

impl TextTargetQuery for SearchFieldQuery<'_> {
    fn target_id(&self) -> TextTargetId {
        self.target
    }

    fn is_active(&self) -> bool {
        self.model.active == Some(self.target)
    }

    fn snapshot(&self) -> EditorSnapshot {
        self.model.snapshot_for(self.target)
    }

    fn profile(&self) -> TextInputProfile {
        TextInputProfile::SearchField
    }

    fn ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
        if self.is_active() {
            self.model.ime_query_target_for(self.target)
        } else {
            None
        }
    }
}

impl TextTargetQuery for SearchActiveOwner<'_> {
    fn target_id(&self) -> TextTargetId {
        self.model.active.unwrap_or(TextTargetId::SearchQuery)
    }

    fn is_active(&self) -> bool {
        self.model.active()
    }

    fn snapshot(&self) -> EditorSnapshot {
        self.model.snapshot_for(self.target_id())
    }

    fn profile(&self) -> TextInputProfile {
        TextInputProfile::SearchField
    }

    fn ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
        self.model
            .active
            .and_then(|target| self.model.ime_query_target_for(target))
    }
}

impl TextTargetOwner for SearchActiveOwner<'_> {
    fn ime_target(&mut self) -> Option<ImeTarget<'_>> {
        match self.model.active {
            Some(TextTargetId::SearchQuery) => Some(self.model.query.as_ime_target()),
            Some(TextTargetId::SearchReplacement) => Some(self.model.replacement.as_ime_target()),
            _ => None,
        }
    }

    fn edit_target(&mut self) -> Option<EditTarget<'_>> {
        self.model.edit_target()
    }
}

impl SearchModel {
    fn snapshot_for(&self, target: TextTargetId) -> EditorSnapshot {
        match target {
            TextTargetId::SearchQuery => self.query.snapshot(),
            TextTargetId::SearchReplacement => self.replacement.snapshot(),
            _ => EditorSnapshot::default(),
        }
    }

    fn ime_query_target_for(&self, target: TextTargetId) -> Option<ImeQueryTarget<'_>> {
        match target {
            TextTargetId::SearchQuery => Some(self.query.as_ime_query_target()),
            TextTargetId::SearchReplacement => Some(self.replacement.as_ime_query_target()),
            _ => None,
        }
    }
}

fn is_search_target(target: TextTargetId) -> bool {
    matches!(
        target,
        TextTargetId::SearchQuery | TextTargetId::SearchReplacement
    )
}
