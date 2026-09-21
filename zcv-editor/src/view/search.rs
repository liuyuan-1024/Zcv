//! Editor 的文件内搜索：持有搜索结果（绑定 BufferVersion），编辑后自动重搜。

use zcv_multi_buffer::{MultiBufferAnchor, MultiBufferRange, MultiBufferSnapshot};

use std::ops::Range;
use std::sync::Arc;

use zcv_project::{RegexSearchResult, SearchQuery, SearchQueryResult, SearchResult};
use zcv_text::{Affinity, BufferVersion};
use zcv_workspace::{Direction, SearchEvent, SearchableItem};

use crate::display_map::SearchDecorationInput;
use crate::selection::EditOutcome;

use super::{Editor, edit_metadata};

impl gpui::EventEmitter<SearchEvent> for Editor {}

/// 搜索结果，literal 与 regex 二选一（均绑定搜索时的 BufferVersion）。
pub(crate) enum SearchResultKind {
    Query(SearchQueryResult),
    External { version: BufferVersion },
}

/// 搜索高亮保存组合文档源锚点；字节范围只在当前快照上按锚点解析。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchMatchAnchor {
    range: Range<MultiBufferAnchor>,
}

impl SearchMatchAnchor {
    pub(crate) fn from_range(snapshot: &MultiBufferSnapshot, range: MultiBufferRange) -> Self {
        Self {
            // 匹配边界不吸收恰好发生在边界上的插入。
            range: snapshot.anchor_at(range.start(), Affinity::After)
                ..snapshot.anchor_at(range.end(), Affinity::Before),
        }
    }

    /// 在当前快照上解析匹配范围；锚点已退出投影或版本无法映射时显式失败。
    fn resolve(&self, snapshot: &MultiBufferSnapshot) -> Option<MultiBufferRange> {
        MultiBufferRange::new(
            snapshot.resolve_anchor(&self.range.start)?,
            snapshot.resolve_anchor(&self.range.end)?,
        )
        .ok()
    }
}

/// Editor 的搜索状态（搜索条执行过一次搜索后存在）。
pub(crate) struct EditorSearch {
    query: SearchQuery,
    result: Option<SearchResultKind>,
    matches: Vec<SearchMatchAnchor>,
    /// `matches` 的字节范围派生缓存；未变化时复用同一 `Arc`，
    /// 让显示链按身份判断搜索装饰输入是否需要重建。
    ranges: Arc<[MultiBufferRange]>,
    active_index: Option<usize>,
}

impl EditorSearch {
    fn matches(&self) -> &[SearchMatchAnchor] {
        &self.matches
    }

    /// 在当前快照上把匹配锚点解析为字节范围派生缓存；无法解析的匹配显式丢弃。
    fn rebuild_ranges(&mut self, snapshot: &MultiBufferSnapshot) {
        let mut matches = Vec::with_capacity(self.matches.len());
        let mut ranges = Vec::with_capacity(self.matches.len());
        for anchor in &self.matches {
            if let Some(range) = anchor.resolve(snapshot) {
                matches.push(anchor.clone());
                ranges.push(range);
            }
        }
        self.matches = matches;
        self.ranges = ranges.into();
        self.active_index = self
            .active_index
            .filter(|index| *index < self.matches.len());
    }

    /// 把当前匹配锚点解析为显示链的搜索装饰输入。
    ///
    /// 锚点权威仍是 Editor 的搜索状态；这里产出的范围只是投影输入，不构成第二份搜索事实。
    pub(super) fn decoration_input(&self) -> Option<SearchDecorationInput> {
        if self.matches.is_empty() {
            return None;
        }
        Some(SearchDecorationInput::new(
            Arc::clone(&self.ranges),
            self.active_index.unwrap_or(0),
        ))
    }

    fn len(&self) -> usize {
        self.matches().len()
    }

    fn match_range(&self, index: usize) -> Range<usize> {
        let range = self.ranges[index];
        range.start().get()..range.end().get()
    }

    fn is_stale(&self, current_version: BufferVersion) -> bool {
        match &self.result {
            Some(SearchResultKind::Query(result)) => result.is_stale(current_version),
            Some(SearchResultKind::External { version }) => *version != current_version,
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
        let range = self.resolved_selections(cx).primary().range();
        if range.is_empty() {
            return None;
        }
        let snapshot = self.display_snapshot(cx).buffer_snapshot().clone();
        Some(
            snapshot
                .bytes_in_range(range.start()..range.end())
                .map(|chunk| chunk.text)
                .collect(),
        )
    }

    fn search(
        &mut self,
        query: &SearchQuery,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.search = self.execute_search(query, cx);
        self.advance_snapshots(cx);
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
        // 先完成本命令的快照推进（可能重建搜索结果），再取推进后的当次结果；
        // 不得把推进前的旧范围交给 select_byte_range，否则会越界或落到错误匹配。
        self.advance_snapshots(cx);
        let Some(range) = self
            .search
            .as_ref()
            .and_then(|search| search.active_index.map(|index| search.match_range(index)))
        else {
            return;
        };
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
        // 搜索结果绑定投影版本；过期校验与直接提交都在权威组合文档侧完成。
        if self.search_result_stale(&literal, &regex, cx) {
            return false;
        }
        let before = self.resolved_selections(cx);
        let metadata = edit_metadata(if literal.is_some() {
            "替换搜索匹配"
        } else {
            "替换正则匹配"
        });
        let outcome = self.change(before, metadata, cx, |buffer| {
            if let Some(result) = literal {
                buffer.replace_search_match(&result, index, replacement)
            } else if let Some(result) = regex {
                buffer.replace_regex_match(&result, index, replacement)
            } else {
                Ok(EditOutcome::unchanged())
            }
        });
        // 只有真正发生替换（事务非空）才视为成功，避免 search_bar 无意义地前移活动匹配。
        outcome.is_ok_and(|outcome| outcome.position_map().is_some())
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
        let before = self.resolved_selections(cx);
        let metadata = edit_metadata(if literal.is_some() {
            "替换全部搜索匹配"
        } else {
            "替换全部正则匹配"
        });
        let outcome = self.change(before, metadata, cx, |buffer| {
            if let Some(result) = literal {
                buffer.replace_all_search_matches(&result, replacement)
            } else if let Some(result) = regex {
                buffer.replace_all_regex_matches(&result, replacement)
            } else {
                Ok(EditOutcome::unchanged())
            }
        });
        let replaced = outcome.is_ok_and(|outcome| outcome.position_map().is_some());
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
        let projection_version = self.display_snapshot(cx).buffer_snapshot().version();
        literal
            .as_ref()
            .is_some_and(|result| result.version() != projection_version)
            || regex
                .as_ref()
                .is_some_and(|result| result.version() != projection_version)
    }

    /// 使用调用方提供的精确范围建立只读搜索高亮，供 MultiBuffer excerpts 等组合结果使用。
    pub(crate) fn set_search_ranges(
        &mut self,
        query: SearchQuery,
        ranges: Vec<MultiBufferRange>,
        cx: &mut gpui::Context<Self>,
    ) {
        let snapshot = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let version = snapshot.version();
        let mut search = EditorSearch {
            query,
            result: Some(SearchResultKind::External { version }),
            matches: ranges
                .into_iter()
                .map(|range| SearchMatchAnchor::from_range(&snapshot, range))
                .collect(),
            ranges: Arc::from([]),
            active_index: None,
        };
        search.rebuild_ranges(&snapshot);
        search.active_index = (!search.matches.is_empty()).then_some(0);
        self.search = Some(search);
        self.advance_snapshots(cx);
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

    /// 将外部搜索结果追加到当前投影，保留已有匹配与活动匹配。
    ///
    /// 结果追加只接受同一查询；
    /// 组合文档增量追加时允许投影版本变化，并在追加前重绑已有范围；
    /// 若调用方尚未建立外部结果，则复用 `set_search_ranges` 建立首批结果。
    pub fn append_search_ranges(
        &mut self,
        query: SearchQuery,
        ranges: Vec<MultiBufferRange>,
        cx: &mut gpui::Context<Self>,
    ) {
        let snapshot = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let version = snapshot.version();
        let can_append = self.search.as_ref().is_some_and(|search| {
            search.query == query
                && matches!(search.result, Some(SearchResultKind::External { .. }))
        });
        if !can_append {
            self.set_search_ranges(query, ranges, cx);
            return;
        }

        let search = self
            .search
            .as_mut()
            .expect("可追加搜索结果时必须存在搜索状态");
        // 已有匹配是源锚点：组合文档追加片段后按当前快照重新解析，不需要按偏移重绑。
        search.result = Some(SearchResultKind::External { version });
        search.matches.extend(
            ranges
                .into_iter()
                .map(|range| SearchMatchAnchor::from_range(&snapshot, range)),
        );
        search.rebuild_ranges(&snapshot);
        self.advance_snapshots(cx);
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
        let virtual_snapshot = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let result = query
            .search_in(
                &virtual_snapshot,
                virtual_snapshot.version(),
                virtual_snapshot.word_boundary(),
            )
            .ok()?;
        let matches = result
            .matches()
            .iter()
            .map(|search_match| {
                SearchMatchAnchor::from_range(&virtual_snapshot, search_match.range().into())
            })
            .collect();
        let mut search = EditorSearch {
            query: query.clone(),
            result: Some(SearchResultKind::Query(result)),
            matches,
            ranges: Arc::from([]),
            active_index: None,
        };
        search.rebuild_ranges(&virtual_snapshot);
        if !search.matches.is_empty() {
            search.active_index = Some(0);
        }
        Some(search)
    }

    /// 编辑事务后调用：本地搜索结果过期时用保存的 query 重搜，活动匹配保持原序号。
    ///
    /// 外部派生结果集（项目搜索结果）由结果所有者拥有，Editor 不把它改写成当前文档上的 Query 重搜；
    /// 只按当前快照重投影已有锚点范围，保持 `External` 语义不变。
    pub(crate) fn research_after_edit(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(search) = &self.search else { return };
        if search.query.query.is_empty() {
            return;
        }
        let snapshot = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let version = snapshot.version();
        if !search.is_stale(version) {
            return;
        }
        let is_external = matches!(search.result, Some(SearchResultKind::External { .. }));
        let query = search.query.clone();
        let active = search.active_index;
        if is_external {
            let Some(search) = self.search.as_mut() else {
                return;
            };
            search.result = Some(SearchResultKind::External { version });
            search.rebuild_ranges(&snapshot);
            cx.notify();
            cx.emit(SearchEvent::MatchesInvalidated);
            return;
        }
        self.search = self.execute_search(&query, cx);
        if let Some(search) = &mut self.search {
            let len = search.len();
            search.active_index = active
                .filter(|index| *index < len)
                .or_else(|| (len > 0).then_some(0));
        }
        cx.notify();
        cx.emit(SearchEvent::MatchesInvalidated);
    }
}

#[cfg(test)]
#[path = "test/search_test.rs"]
mod test;
