use zom_command::{EditTarget, SearchOption, SearchScope};
use zom_engine::{
    ByteOffset, RegexSearchOptions as EngineRegexSearchOptions, Selection, SelectionSet, TextRange,
};
use zom_view::{RevealKind, ViewSet};
use zom_workspace::{BufferId, Workspace};

use super::query::{buffer_title, literal_search_options, regex_pattern, search_result_location};
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

    pub(crate) fn set_scope(&mut self, scope: SearchScope, workspace: &Workspace, views: &ViewSet) {
        self.scope = scope;
        self.refresh(workspace, views, None);
    }

    pub(crate) fn toggle_option(
        &mut self,
        option: SearchOption,
        workspace: &Workspace,
        views: &ViewSet,
    ) {
        match option {
            SearchOption::CaseSensitive => {
                self.options.case_sensitive = !self.options.case_sensitive
            }
            SearchOption::WholeWord => self.options.whole_word = !self.options.whole_word,
            SearchOption::Regex => self.options.regex = !self.options.regex,
        }
        self.refresh(workspace, views, None);
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

/// 搜索 / 替换的核心动作。
///
/// 这些方法都吃 `&mut Workspace, &mut ViewSet`（或不可变借用）—— 与
/// [`FileTreeModel`](super::super::file_tree::FileTreeModel) 的风格一致，宿主只做
/// 字段级转发，不再在组合根里铺 200+ 行搜索流程。
///
/// [`FileTreeModel`]: super::super::file_tree::FileTreeModel
impl SearchModel {
    /// 重新搜索：清空旧结果并以当前 query / options / scope 重跑。
    ///
    /// `requested_active` 用来在导航 / 替换之后明确指定停在第几条；为 `None`
    /// 时尽量保留旧的活动项，落空则回退到第一条。
    pub(crate) fn refresh(
        &mut self,
        workspace: &Workspace,
        views: &ViewSet,
        requested_active: Option<usize>,
    ) {
        let query = self.query_text();
        if query.is_empty() {
            self.clear_results();
            return;
        }

        let options = self.options;
        let mut results = Vec::new();
        let mut error = None;
        for id in self.target_buffer_ids(workspace, views) {
            let Some(buffer) = workspace.buffer(id) else {
                continue;
            };
            let text = buffer.buffer().text().into_owned();
            let ranges = if options.regex {
                let pattern = regex_pattern(&query, options.whole_word);
                match buffer.buffer().search_regex(
                    &pattern,
                    EngineRegexSearchOptions::new().with_case_sensitive(options.case_sensitive),
                ) {
                    Ok(result) => result.ranges().collect::<Vec<_>>(),
                    Err(search_error) => {
                        error = Some(format!("搜索失败：{search_error}"));
                        Vec::new()
                    }
                }
            } else {
                match buffer.buffer().search(&query, literal_search_options(options)) {
                    Ok(result) => result.ranges().collect::<Vec<_>>(),
                    Err(search_error) => {
                        error = Some(format!("搜索失败：{search_error}"));
                        Vec::new()
                    }
                }
            };
            if error.is_some() {
                break;
            }
            let title = buffer_title(buffer);
            for (buffer_ordinal, range) in ranges.into_iter().enumerate() {
                let (line, column, preview) = search_result_location(&text, range);
                results.push(SearchResultItem {
                    buffer_id: id,
                    title: title.clone(),
                    range,
                    buffer_ordinal,
                    line,
                    column,
                    preview,
                });
            }
        }

        let active = requested_active
            .or(self.active_result)
            .filter(|index| *index < results.len())
            .or_else(|| (!results.is_empty()).then_some(0));
        self.set_results(results, active, error);
    }

    /// IME / 命令路径调完之后统一调它：query 文本变了才触发一次 refresh。
    pub(crate) fn refresh_if_query_changed(&mut self, workspace: &Workspace, views: &ViewSet) {
        if self.query_changed_since_last_sync() {
            self.refresh(workspace, views, None);
        }
    }

    pub(crate) fn find_next(&mut self, workspace: &mut Workspace, views: &mut ViewSet) {
        self.refresh(workspace, views, None);
        let len = self.results.len();
        if len == 0 {
            return;
        }
        let next = self
            .active_result
            .map(|index| (index + 1) % len)
            .unwrap_or(0);
        self.refresh(workspace, views, Some(next));
        self.select_active_result(workspace, views);
    }

    pub(crate) fn find_previous(&mut self, workspace: &mut Workspace, views: &mut ViewSet) {
        self.refresh(workspace, views, None);
        let len = self.results.len();
        if len == 0 {
            return;
        }
        let previous = self
            .active_result
            .map(|index| if index == 0 { len - 1 } else { index - 1 })
            .unwrap_or(0);
        self.refresh(workspace, views, Some(previous));
        self.select_active_result(workspace, views);
    }

    pub(crate) fn replace_next(&mut self, workspace: &mut Workspace, views: &mut ViewSet) {
        self.refresh(workspace, views, None);
        let Some(active_index) = self.active_result else {
            return;
        };
        let Some(item) = self.results.get(active_index).cloned() else {
            return;
        };
        let query = self.query_text();
        let replacement = self.replacement_text();
        let options = self.options;
        let Some(buffer) = workspace.buffer_mut(item.buffer_id) else {
            return;
        };
        let result = if options.regex {
            let pattern = regex_pattern(&query, options.whole_word);
            buffer
                .buffer_mut()
                .search_regex(
                    &pattern,
                    EngineRegexSearchOptions::new().with_case_sensitive(options.case_sensitive),
                )
                .and_then(|result| {
                    buffer
                        .buffer_mut()
                        .replace_regex_match(&result, item.buffer_ordinal, &replacement)
                        .map(|_| ())
                })
        } else {
            buffer
                .buffer_mut()
                .search(&query, literal_search_options(options))
                .and_then(|result| {
                    buffer
                        .buffer_mut()
                        .replace_search_match(&result, item.buffer_ordinal, &replacement)
                        .map(|_| ())
                })
        };
        if let Err(error) = result {
            self.set_results(Vec::new(), None, Some(format!("替换失败：{error}")));
            return;
        }
        self.refresh(workspace, views, Some(active_index));
        self.select_active_result(workspace, views);
    }

    pub(crate) fn replace_all(&mut self, workspace: &mut Workspace, views: &mut ViewSet) {
        self.refresh(workspace, views, None);
        let query = self.query_text();
        if query.is_empty() {
            return;
        }
        let replacement = self.replacement_text();
        let options = self.options;
        let ids = self.target_buffer_ids(workspace, views);
        let active_buffer_id = views.active_view().map(|view| view.buffer());
        // 替换之后没有「下一个 active match」可言；把光标停在活动 buffer 内
        // 最后一处替换的末尾，并 reveal 那里 —— 这是替换后唯一有意义的焦点。
        let mut active_reveal: Option<ByteOffset> = None;
        for id in ids {
            let Some(buffer) = workspace.buffer_mut(id) else {
                continue;
            };
            let outcome = if options.regex {
                let pattern = regex_pattern(&query, options.whole_word);
                buffer
                    .buffer_mut()
                    .search_regex(
                        &pattern,
                        EngineRegexSearchOptions::new().with_case_sensitive(options.case_sensitive),
                    )
                    .and_then(|result| {
                        buffer
                            .buffer_mut()
                            .replace_all_regex_matches(&result, &replacement)
                    })
            } else {
                buffer
                    .buffer_mut()
                    .search(&query, literal_search_options(options))
                    .and_then(|result| {
                        buffer
                            .buffer_mut()
                            .replace_all_search_matches(&result, &replacement)
                    })
            };
            match outcome {
                Ok(Some((_, changeset))) if Some(id) == active_buffer_id => {
                    // ChangeSet 已经按 post-replacement 坐标列出每次编辑的落点，
                    // 直接拿最后一项的 end 即"最后那次替换的末尾"，不必自己算累积偏移。
                    match changeset.changed_ranges() {
                        Ok(ranges) => {
                            if let Some(last) = ranges.last() {
                                active_reveal = Some(last.end());
                            }
                        }
                        Err(error) => {
                            self.set_results(
                                Vec::new(),
                                None,
                                Some(format!("替换失败：{error}")),
                            );
                            return;
                        }
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    self.set_results(Vec::new(), None, Some(format!("替换失败：{error}")));
                    return;
                }
            }
        }
        if let Some(byte) = active_reveal
            && let Some(view) = views.active_view_mut()
        {
            *view.selection_mut() = SelectionSet::new(vec![Selection::caret(byte)]);
            view.request_reveal(byte, RevealKind::Match);
        }
        self.refresh(workspace, views, None);
    }

    fn target_buffer_ids(&self, workspace: &Workspace, views: &ViewSet) -> Vec<BufferId> {
        match self.scope {
            SearchScope::CurrentFile => views
                .active_view()
                .map(|view| vec![view.buffer()])
                .unwrap_or_default(),
            SearchScope::Project => workspace.buffers().map(|(id, _)| id).collect::<Vec<_>>(),
        }
    }

    fn select_active_result(&self, workspace: &mut Workspace, views: &mut ViewSet) {
        let Some(item) = self
            .active_result
            .and_then(|index| self.results.get(index))
            .cloned()
        else {
            return;
        };
        let _ = workspace.set_active_buffer(item.buffer_id);
        let existing_view = views
            .views()
            .find_map(|(id, view)| (view.buffer() == item.buffer_id).then_some(id));
        let view_id = match existing_view {
            Some(id) => id,
            None => {
                let Some(buffer) = workspace.buffer(item.buffer_id) else {
                    return;
                };
                let version = buffer.buffer().version();
                views.open_view(item.buffer_id, version)
            }
        };
        views.set_active(view_id);
        if let Some(view) = views.active_view_mut() {
            *view.selection_mut() =
                SelectionSet::new(vec![Selection::new(item.range.start(), item.range.end())]);
            // 用 `RevealKind::Match` —— 当前 match 已在视区内就不动；
            // 后续做了 active match 高亮后，这条「视区不跳」就成了自然行为。
            view.request_reveal(item.range.start(), RevealKind::Match);
        }
    }
}
