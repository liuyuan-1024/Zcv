//! Editor 的文件内搜索：持有搜索结果（绑定 BufferVersion），编辑后自动重搜。

use zcv_multi_buffer::{MultiBufferOffset, MultiBufferRange};

use std::ops::Range;
use std::sync::{Arc, Mutex};

use gpui::{Bounds, Pixels};
use zcv_project::{RegexSearchResult, SearchQuery, SearchQueryResult, SearchResult};
use zcv_text::{Affinity, Anchor, BufferVersion, PositionMap};
use zcv_workspace::{Direction, SearchEvent, SearchableItem};

use crate::display_map::{DisplayRange, DisplaySnapshot};
use crate::scrollbar::{ScrollbarMarker, ScrollbarMarkerKind, marker_geometry};
use crate::selection::EditOutcome;

use super::{Editor, edit_metadata};

impl gpui::EventEmitter<SearchEvent> for Editor {}

/// 搜索结果，literal 与 regex 二选一（均绑定搜索时的 BufferVersion）。
pub(crate) enum SearchResultKind {
    Query(SearchQueryResult),
    External { version: BufferVersion },
}

/// 搜索高亮保存稳定锚点；字节范围只在执行搜索时用于生成锚点。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchMatchAnchor {
    range: Range<Anchor>,
}

#[derive(Clone, Copy, PartialEq)]
struct MarkerGeometryKey {
    track_top: f32,
    track_height: f32,
    scroll_per_pixel: f32,
    line_height: f32,
}

/// 绑定搜索状态与显示拓扑版本的不可变装饰快照。
///
/// 视口高亮按字节范围 seek 后连续消费；滚动栏行投影只在快照建立时计算一次。
pub(crate) struct SearchDecorationSnapshot {
    ranges: Arc<[MultiBufferRange]>,
    active_index: usize,
    projected_rows: Arc<[Range<usize>]>,
    markers: Mutex<Option<(MarkerGeometryKey, Arc<[ScrollbarMarker]>)>>,
}

impl SearchDecorationSnapshot {
    fn new(display: &DisplaySnapshot, search: &EditorSearch) -> Self {
        let ranges = search
            .matches()
            .iter()
            .map(SearchMatchAnchor::range)
            .collect::<Arc<[_]>>();
        Self::from_ranges(display, ranges, search.active_index.unwrap_or(0))
    }

    fn from_ranges(
        display: &DisplaySnapshot,
        ranges: Arc<[MultiBufferRange]>,
        active_index: usize,
    ) -> Self {
        let projected_rows = ranges
            .iter()
            .flat_map(|range| display.project_text_range(*range).unwrap_or_default())
            .map(projected_row_range)
            .collect::<Arc<[_]>>();
        Self {
            ranges,
            active_index,
            projected_rows,
            markers: Mutex::new(None),
        }
    }

    pub(crate) fn visible_ranges(
        &self,
        viewport: Range<usize>,
    ) -> impl Iterator<Item = (usize, MultiBufferRange)> + '_ {
        let start = self
            .ranges
            .partition_point(|range| range.end().get() <= viewport.start);
        self.ranges[start..]
            .iter()
            .enumerate()
            .take_while(move |(_, range)| range.start().get() < viewport.end)
            .map(move |(index, range)| (start + index, *range))
    }

    pub(crate) fn is_active(&self, index: usize) -> bool {
        index == self.active_index
    }

    pub(crate) fn scrollbar_markers(
        &self,
        track_bounds: Bounds<Pixels>,
        scroll_per_pixel: f32,
        line_height: Pixels,
    ) -> Arc<[ScrollbarMarker]> {
        let key = MarkerGeometryKey {
            track_top: f32::from(track_bounds.top()),
            track_height: f32::from(track_bounds.size.height),
            scroll_per_pixel,
            line_height: f32::from(line_height),
        };
        let mut cache = self.markers.lock().expect("搜索标记缓存锁不应中毒");
        if let Some((cached_key, markers)) = &*cache
            && *cached_key == key
        {
            return Arc::clone(markers);
        }
        let markers = Arc::from(
            marker_geometry(
                self.projected_rows
                    .iter()
                    .cloned()
                    .map(|rows| (rows, ScrollbarMarkerKind::Search)),
                track_bounds,
                scroll_per_pixel,
                line_height,
            )
            .into_boxed_slice(),
        );
        *cache = Some((key, Arc::clone(&markers)));
        markers
    }
}

fn projected_row_range(range: DisplayRange) -> Range<usize> {
    let start = range.start();
    let end = range.end();
    let end_line = if end.row() == start.row() || end.column().get() != 0 {
        end.row().get().saturating_add(1)
    } else {
        end.row().get()
    };
    start.row().get()..end_line
}

impl SearchMatchAnchor {
    pub(crate) fn from_range(version: BufferVersion, range: MultiBufferRange) -> Self {
        Self {
            // 匹配边界不吸收恰好发生在边界上的插入。
            range: Anchor::new(version, range.start().into()).with_affinity(Affinity::After)
                ..Anchor::new(version, range.end().into()).with_affinity(Affinity::Before),
        }
    }

    pub(crate) fn range(&self) -> MultiBufferRange {
        MultiBufferRange::new(
            MultiBufferOffset::new(self.range.start.offset().get()),
            MultiBufferOffset::new(self.range.end.offset().get()),
        )
        .expect("搜索匹配锚点范围必须有序")
    }
}

/// Editor 的搜索状态（搜索条执行过一次搜索后存在）。
pub(crate) struct EditorSearch {
    query: SearchQuery,
    result: Option<SearchResultKind>,
    matches: Vec<SearchMatchAnchor>,
    active_index: Option<usize>,
}

impl EditorSearch {
    fn matches(&self) -> &[SearchMatchAnchor] {
        &self.matches
    }

    fn len(&self) -> usize {
        self.matches().len()
    }

    fn match_range(&self, index: usize) -> Range<usize> {
        let range = self.matches()[index].range();
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
        let range = self.resolved_selections().primary().range();
        if range.is_empty() {
            return None;
        }
        let snapshot = self.multi_buffer.read(cx).snapshot(cx);
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
        self.invalidate_search_decorations();
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
        self.invalidate_search_decorations();
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
        self.invalidate_search_decorations();
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
        let before = self.resolved_selections();
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
        let before = self.resolved_selections();
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
    fn invalidate_search_decorations(&mut self) {
        self.search_revision = self.search_revision.wrapping_add(1);
        self.search_decorations = None;
    }

    pub(crate) fn search_decorations(
        &mut self,
        display: &DisplaySnapshot,
    ) -> Option<Arc<SearchDecorationSnapshot>> {
        let search = self.search.as_ref()?;
        if search.len() == 0 {
            return None;
        }
        let key = super::SearchDecorationKey {
            display_revision: display.revision(),
            search_revision: self.search_revision,
        };
        if let Some((cached_key, decorations)) = &self.search_decorations
            && *cached_key == key
        {
            return Some(Arc::clone(decorations));
        }
        let decorations = Arc::new(SearchDecorationSnapshot::new(display, search));
        self.search_decorations = Some((key, Arc::clone(&decorations)));
        Some(decorations)
    }

    /// 在搜索协调器完成重算前，先让已有高亮随同一批文本变化移动。
    ///
    /// 这只维护已有范围的位置；匹配是否仍然存在，仍由随后基于当前快照的重算决定。
    pub(crate) fn map_search_anchors(
        &mut self,
        old_version: BufferVersion,
        new_version: BufferVersion,
        position_map: &PositionMap,
    ) {
        let Some(search) = &mut self.search else {
            return;
        };
        let mut changed = false;
        for search_match in &mut search.matches {
            if search_match.range.start.version() != old_version
                || search_match.range.end.version() != old_version
            {
                continue;
            }
            search_match.range = search_match
                .range
                .start
                .map_through_position_map(new_version, position_map)
                .value()
                ..search_match
                    .range
                    .end
                    .map_through_position_map(new_version, position_map)
                    .value();
            changed = true;
        }
        if changed {
            self.invalidate_search_decorations();
        }
    }

    /// 搜索结果是否已偏离当前投影版本（过期校验在搜索绑定的权威文档侧完成）。
    fn search_result_stale(
        &self,
        literal: &Option<SearchResult>,
        regex: &Option<RegexSearchResult>,
        cx: &gpui::Context<Self>,
    ) -> bool {
        let projection_version = self.multi_buffer.read(cx).snapshot(cx).version();
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
        let version = self.multi_buffer.read(cx).snapshot(cx).version();
        let matches = ranges
            .into_iter()
            .map(|range| SearchMatchAnchor::from_range(version, range))
            .collect::<Vec<_>>();
        let active_index = (!matches.is_empty()).then_some(0);
        self.search = Some(EditorSearch {
            query,
            result: Some(SearchResultKind::External { version }),
            matches,
            active_index,
        });
        self.invalidate_search_decorations();
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
        let version = self.multi_buffer.read(cx).snapshot(cx).version();
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
        // 组合文档按批次追加片段时，投影版本会随每次追加递增；
        // 已有匹配的字节偏移不变，但必须重绑到最新版本，否则下一批会被误判为新搜索。
        for search_match in &mut search.matches {
            let range = search_match.range();
            *search_match = SearchMatchAnchor::from_range(version, range);
        }
        search.result = Some(SearchResultKind::External { version });
        search.matches.extend(
            ranges
                .into_iter()
                .map(|range| SearchMatchAnchor::from_range(version, range)),
        );
        self.invalidate_search_decorations();
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
        let virtual_snapshot = self.multi_buffer.read(cx).snapshot(cx);
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
                SearchMatchAnchor::from_range(
                    virtual_snapshot.version(),
                    search_match.range().into(),
                )
            })
            .collect();
        let search = EditorSearch {
            query: query.clone(),
            result: Some(SearchResultKind::Query(result)),
            matches,
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
        let version = self.multi_buffer.read(cx).snapshot(cx).version();
        if !search.is_stale(version) {
            return;
        }
        let query = search.query.clone();
        let active = search.active_index;
        self.search = self.execute_search(&query, cx);
        if let Some(search) = &mut self.search {
            let len = search.len();
            search.active_index = active
                .filter(|index| *index < len)
                .or_else(|| (len > 0).then_some(0));
        }
        self.invalidate_search_decorations();
        cx.notify();
        cx.emit(SearchEvent::MatchesInvalidated);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    impl SearchDecorationSnapshot {
        pub(crate) fn for_test(
            display: &DisplaySnapshot,
            ranges: &[MultiBufferRange],
            active_index: usize,
        ) -> Self {
            Self::from_ranges(display, Arc::from(ranges), active_index)
        }

        pub(crate) fn projected_rows_for_test(&self) -> &[Range<usize>] {
            &self.projected_rows
        }
    }

    impl Editor {
        pub(crate) fn search_highlights(&self) -> Option<(&[SearchMatchAnchor], usize)> {
            let search = self.search.as_ref()?;
            if search.len() == 0 {
                return None;
            }
            Some((search.matches(), search.active_index.unwrap_or(0)))
        }
    }
}
