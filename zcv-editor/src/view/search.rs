//! Editor 的文件内搜索：持有搜索结果（绑定 BufferVersion），编辑后自动重搜。

use std::ops::Range;

use zcv_text::{
    BufferVersion, RegexSearchResult, SearchMatch, SearchQuery, SearchQueryResult, SearchResult,
    TextRange,
};
use zcv_workspace::{Direction, SearchEvent, SearchableItem};

use crate::selection::EditOutcome;

use super::{Editor, edit_metadata};

impl gpui::EventEmitter<SearchEvent> for Editor {}

/// 搜索结果，literal 与 regex 二选一（均绑定搜索时的 BufferVersion）。
pub(crate) enum SearchResultKind {
    Query(SearchQueryResult),
    External {
        version: BufferVersion,
        matches: Vec<SearchMatch>,
    },
}

/// Editor 的搜索状态（搜索条执行过一次搜索后存在）。
pub(crate) struct EditorSearch {
    query: SearchQuery,
    result: Option<SearchResultKind>,
    active_index: Option<usize>,
}

impl EditorSearch {
    fn matches(&self) -> &[SearchMatch] {
        match &self.result {
            Some(SearchResultKind::Query(result)) => result.matches(),
            Some(SearchResultKind::External { matches, .. }) => matches,
            None => &[],
        }
    }

    fn len(&self) -> usize {
        self.matches().len()
    }

    fn match_range(&self, index: usize) -> Range<usize> {
        let range = self.matches()[index].range();
        range.start().get()..range.end().get()
    }

    fn is_stale(&self, version: zcv_text::BufferVersion) -> bool {
        match &self.result {
            Some(SearchResultKind::Query(result)) => result.is_stale(version),
            Some(SearchResultKind::External {
                version: result_version,
                ..
            }) => *result_version != version,
            None => false,
        }
    }

    /// 拆分搜索结果中的 literal / regex 变体（编辑闭包按类型分派）。
    fn cloned_result(&self) -> (Option<SearchResult>, Option<RegexSearchResult>) {
        match &self.result {
            Some(SearchResultKind::Query(SearchQueryResult::Literal(result))) => {
                (Some(result.clone()), None)
            }
            Some(SearchResultKind::Query(SearchQueryResult::Regex(result))) => {
                (None, Some(result.clone()))
            }
            Some(SearchResultKind::External { .. }) => (None, None),
            None => (None, None),
        }
    }
}

impl SearchableItem for Editor {
    /// 主选区文本作为查询建议；空选区（仅光标）不种入。
    fn query_suggestion(&self, cx: &gpui::App) -> Option<String> {
        let range = self.resolved_selections().primary().range();
        if range.is_empty() {
            return None;
        }
        let snapshot = self.text_buffer(cx).read(cx).snapshot();
        Some(
            snapshot
                .slice_byte_range(range.start(), range.end())
                .expect("主选区范围在当前投影快照内必须可读取")
                .as_str()
                .to_owned(),
        )
    }

    fn search(
        &mut self,
        query: &SearchQuery,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.search = self.execute_search(query, cx);
        // 自动定位到第一个匹配（选区 + 视口滚动，光标跟随）。
        if let Some(search) = &self.search
            && let Some(index) = search.active_index
        {
            let range = search.match_range(index);
            self.select_byte_range(range, cx);
        }
        // 立即重绘：高亮与定位不等待下一次交互触发（输入即高亮）。
        cx.notify();
        cx.emit(SearchEvent::MatchesInvalidated);
    }

    fn clear_search(&mut self, _window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        self.search = None;
        cx.notify();
        cx.emit(SearchEvent::MatchesInvalidated);
    }

    fn search_count(&self, _cx: &gpui::App) -> (usize, Option<usize>) {
        self.search
            .as_ref()
            .map_or((0, None), |search| (search.len(), search.active_index))
    }

    fn activate_match_in_direction(
        &mut self,
        direction: Direction,
        count: usize,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(search) = &mut self.search else {
            return;
        };
        let len = search.len();
        if len == 0 {
            return;
        }
        let current = search.active_index.unwrap_or(0);
        let next = match direction {
            Direction::Next => (current + count) % len,
            Direction::Prev => (current + len - count % len) % len,
        };
        search.active_index = Some(next);
        let range = search.match_range(next);
        self.select_byte_range(range, cx);
        cx.emit(SearchEvent::ActiveMatchChanged);
    }

    fn replace_current(
        &mut self,
        replacement: &str,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> bool {
        // 编辑链路会在事务后自动重搜，这里兜底拒绝过期结果。
        let Some(search) = &self.search else {
            return false;
        };
        let Some(index) = search.active_index else {
            return false;
        };
        let (literal, regex) = search.cloned_result();
        // 搜索结果绑定投影版本：过期校验在权威文档侧完成，编辑 planner 是与投影文本一致的 scratch 副本，重绑后继承坐标。
        if self.search_result_stale(&literal, &regex, cx) {
            return false;
        }
        let before = self.resolved_selections();
        let metadata = edit_metadata(if literal.is_some() {
            "替换搜索匹配"
        } else {
            "替换正则匹配"
        });
        let outcome = self.change(before, metadata, cx, |buffer| {
            if let Some(result) = literal {
                buffer
                    .replace_search_match(
                        &result.rebinding_to(buffer.version()),
                        index,
                        replacement,
                    )
                    .map(EditOutcome::from_transaction)
            } else if let Some(result) = regex {
                buffer
                    .replace_regex_match(&result.rebinding_to(buffer.version()), index, replacement)
                    .map(EditOutcome::from_transaction)
            } else {
                Ok(EditOutcome::unchanged())
            }
        });
        // 只有真正发生替换（事务非空）才视为成功，避免 search_bar 无意义地前移活动匹配。
        outcome.is_ok_and(|outcome| outcome.transaction().is_some())
    }

    fn replace_all(
        &mut self,
        replacement: &str,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> usize {
        let Some(search) = &self.search else { return 0 };
        let count = search.len();
        let (literal, regex) = search.cloned_result();
        if self.search_result_stale(&literal, &regex, cx) {
            return 0;
        }
        let before = self.resolved_selections();
        let metadata = edit_metadata(if literal.is_some() {
            "替换全部搜索匹配"
        } else {
            "替换全部正则匹配"
        });
        let outcome = self.change(before, metadata, cx, |buffer| {
            if let Some(result) = literal {
                buffer
                    .replace_all_search_matches(&result.rebinding_to(buffer.version()), replacement)
                    .map(EditOutcome::from_transaction)
            } else if let Some(result) = regex {
                buffer
                    .replace_all_regex_matches(&result.rebinding_to(buffer.version()), replacement)
                    .map(EditOutcome::from_transaction)
            } else {
                Ok(EditOutcome::unchanged())
            }
        });
        let replaced = outcome.is_ok_and(|outcome| outcome.transaction().is_some());
        if replaced { count } else { 0 }
    }
}

impl Editor {
    /// 搜索结果是否已偏离当前投影版本（过期校验在搜索绑定的权威文档侧完成）。
    fn search_result_stale(
        &self,
        literal: &Option<SearchResult>,
        regex: &Option<RegexSearchResult>,
        cx: &gpui::Context<Self>,
    ) -> bool {
        let projection_version = self.text_buffer(cx).read(cx).version();
        literal
            .as_ref()
            .is_some_and(|result| result.version() != projection_version)
            || regex
                .as_ref()
                .is_some_and(|result| result.version() != projection_version)
    }

    /// 使用调用方提供的精确范围建立只读搜索高亮，供 MultiBuffer excerpts 等组合结果使用。
    pub fn set_search_ranges(
        &mut self,
        query: SearchQuery,
        ranges: Vec<TextRange>,
        cx: &mut gpui::Context<Self>,
    ) {
        let version = self.text_buffer(cx).read(cx).snapshot().version();
        let matches = ranges
            .into_iter()
            .enumerate()
            .map(|(ordinal, range)| SearchMatch::new(ordinal, range))
            .collect::<Vec<_>>();
        let active_index = (!matches.is_empty()).then_some(0);
        self.search = Some(EditorSearch {
            query,
            result: Some(SearchResultKind::External { version, matches }),
            active_index,
        });
        if let Some(range) = self
            .search
            .as_ref()
            .and_then(|search| search.active_index.map(|index| search.match_range(index)))
        {
            self.select_byte_range(range, cx);
        }
        cx.notify();
        cx.emit(SearchEvent::MatchesInvalidated);
    }

    /// 执行搜索并返回新的搜索状态；`None` 表示无结果（query 为空或搜索报错）。
    fn execute_search(
        &self,
        query: &SearchQuery,
        cx: &mut gpui::Context<Self>,
    ) -> Option<EditorSearch> {
        if query.query.is_empty() {
            return None;
        }
        let snapshot = self.text_buffer(cx).read(cx).snapshot();
        let result = query.search(&snapshot).ok().map(SearchResultKind::Query)?;
        let search = EditorSearch {
            query: query.clone(),
            result: Some(result),
            active_index: None,
        };
        let search = if search.matches().is_empty() {
            search
        } else {
            EditorSearch {
                active_index: Some(0),
                ..search
            }
        };
        Some(search)
    }

    /// 编辑事务后调用：搜索结果过期时用保存的 query 重搜，活动匹配保持原序号。
    pub(crate) fn research_after_edit(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(search) = &self.search else { return };
        if search.query.query.is_empty() {
            return;
        }
        let version = self.text_buffer(cx).read(cx).snapshot().version();
        if !search.is_stale(version) {
            return;
        }
        let query = search.query.clone();
        let active = search.active_index;
        self.search = self.execute_search(&query, cx);
        if let Some(search) = &mut self.search {
            let len = search.len();
            search.active_index = active.filter(|index| *index < len);
        }
        cx.notify();
        cx.emit(SearchEvent::MatchesInvalidated);
    }

    /// 搜索状态存在且非空时的匹配高亮（供 element 渲染层读取）。
    pub(crate) fn search_highlights(&self) -> Option<(&[SearchMatch], usize)> {
        let search = self.search.as_ref()?;
        if search.len() == 0 {
            return None;
        }
        Some((search.matches(), search.active_index.unwrap_or(0)))
    }
}
