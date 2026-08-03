use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use tree_sitter::{InputEdit, Parser, Point, QueryCursor, StreamingIterator};
use zcv_engine::{BufferVersion, ByteOffset, Snapshot, TextChangeBatch};

use crate::Language;
use crate::language::{language_for_file, language_for_injection};

/// 一个非重叠的 tree-sitter capture 区间。
///
/// `capture` 是快照全局 capture 名字表的索引（跨主语言与注入语言唯一），
/// 渲染侧按索引查预展开的样式表，不再携带并逐 run 解析 capture 名。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub capture: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxLayerInfo {
    pub language: &'static str,
    pub range: Range<usize>,
    pub depth: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BracketPair {
    pub open: Range<usize>,
    pub close: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineItem {
    pub range: Range<usize>,
    pub name_ranges: Vec<Range<usize>>,
    pub context_ranges: Vec<Range<usize>>,
    pub body_range: Option<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndentRange {
    pub range: Range<usize>,
    pub end: Option<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextObjectRange {
    pub kind: Arc<str>,
    pub range: Range<usize>,
}

/// 可增量更新的语法状态。
///
/// `parsed_version` 表示 Tree 真正完成解析的版本；
/// `interpolated_version` 表示旧树已经通过 `InputEdit` 推进到的文本版本。
/// 两者分离后，前台可以立即使用坐标正确的旧树，真正的增量解析则交给后台完成。
pub struct SyntaxMap {
    language: Option<Language>,
    tree: Option<tree_sitter::Tree>,
    injections: Vec<SyntaxLayer>,
    parsed_version: BufferVersion,
    interpolated_version: BufferVersion,
    update_count: u64,
    /// 最近一次解析安装的 capture 全局表（见 `SyntaxSnapshot::rebuild_capture_table`）。
    capture_names: Arc<[Arc<str>]>,
    capture_index_by_name: Arc<HashMap<Arc<str>, u32>>,
}

/// 与一个 Buffer 版本绑定的不可变语法快照。
#[derive(Clone)]
pub struct SyntaxSnapshot {
    language: Option<Language>,
    tree: Option<tree_sitter::Tree>,
    injections: Vec<SyntaxLayer>,
    version: BufferVersion,
    update_count: u64,
    /// 主语言与全部注入语言的 capture 名字全局表；index 跨语言唯一。
    capture_names: Arc<[Arc<str>]>,
    /// capture 名 -> 全局索引的反查表。
    capture_index_by_name: Arc<HashMap<Arc<str>, u32>>,
}

#[derive(Clone)]
struct SyntaxLayer {
    language: Language,
    tree: tree_sitter::Tree,
    range: Range<usize>,
    depth: u32,
}

impl SyntaxMap {
    pub fn new(snapshot: &Snapshot) -> Self {
        Self {
            language: None,
            tree: None,
            injections: Vec::new(),
            parsed_version: snapshot.version(),
            interpolated_version: snapshot.version(),
            update_count: 0,
            capture_names: Arc::from([]),
            capture_index_by_name: Arc::new(HashMap::new()),
        }
    }

    pub fn set_language_for_file(&mut self, path: &Path, snapshot: &Snapshot) -> bool {
        let first_line = snapshot
            .slice_line(zcv_engine::Line::ZERO)
            .ok()
            .map(|line| line.as_str().trim_end_matches(['\r', '\n']).to_owned());
        self.set_language(language_for_file(path, first_line.as_deref()), snapshot)
    }

    pub fn set_language(&mut self, language: Option<Language>, snapshot: &Snapshot) -> bool {
        let unchanged =
            self.language.as_ref().map(Language::name) == language.as_ref().map(Language::name);
        if unchanged {
            return false;
        }
        self.language = language;
        self.tree = None;
        self.injections.clear();
        self.capture_names = Arc::from([]);
        self.capture_index_by_name = Arc::new(HashMap::new());
        self.parsed_version = snapshot.version();
        self.interpolated_version = snapshot.version();
        self.update_count += 1;
        true
    }

    /// 只把旧树推进到新坐标，不在调用线程执行解析。
    pub fn interpolate(
        &mut self,
        old_snapshot: &Snapshot,
        new_snapshot: &Snapshot,
        changes: &TextChangeBatch,
    ) {
        if self.language.is_none() {
            self.parsed_version = new_snapshot.version();
            self.interpolated_version = new_snapshot.version();
            return;
        }

        let can_increment = !changes.requires_reset()
            && changes.old_version() == Some(self.interpolated_version)
            && changes.new_version() == Some(new_snapshot.version())
            && old_snapshot.version() == self.interpolated_version;

        let mut tree = self.tree.take();
        if can_increment {
            if tree
                .as_mut()
                .is_some_and(|tree| !edit_tree(tree, old_snapshot, new_snapshot, changes))
            {
                tree = None;
            }
            self.injections.retain_mut(|layer| {
                if !edit_tree(&mut layer.tree, old_snapshot, new_snapshot, changes) {
                    return false;
                }
                layer.range = map_range_through_changes(layer.range.clone(), changes);
                layer.range.start < layer.range.end
            });
        } else {
            tree = None;
            self.injections.clear();
        }

        self.tree = tree;
        self.interpolated_version = new_snapshot.version();
    }

    pub fn snapshot(&self) -> SyntaxSnapshot {
        SyntaxSnapshot {
            language: self.language.clone(),
            tree: self.tree.clone(),
            injections: self.injections.clone(),
            version: self.interpolated_version,
            update_count: self.update_count,
            capture_names: Arc::clone(&self.capture_names),
            capture_index_by_name: Arc::clone(&self.capture_index_by_name),
        }
    }

    pub fn did_parse(&mut self, parsed: SyntaxSnapshot) -> bool {
        if parsed.version != self.interpolated_version
            || parsed.language.as_ref().map(Language::name)
                != self.language.as_ref().map(Language::name)
        {
            return false;
        }
        self.tree = parsed.tree;
        self.injections = parsed.injections;
        self.capture_names = parsed.capture_names;
        self.capture_index_by_name = parsed.capture_index_by_name;
        self.parsed_version = parsed.version;
        self.update_count += 1;
        true
    }
}

impl SyntaxSnapshot {
    pub fn version(&self) -> BufferVersion {
        self.version
    }

    pub fn update_count(&self) -> u64 {
        self.update_count
    }

    pub fn has_language(&self) -> bool {
        self.language.is_some()
    }

    pub fn syntax_layers(&self, range: Range<usize>, text: &Snapshot) -> Vec<SyntaxLayerInfo> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }
        self.layers_for_range(&range)
            .into_iter()
            .map(|layer| SyntaxLayerInfo {
                language: layer.language.name(),
                range: layer.range,
                depth: layer.depth,
            })
            .collect()
    }

    pub fn language_at(&self, offset: usize, text: &Snapshot) -> Option<&'static str> {
        let range = offset..offset;
        self.can_query(&range, text).then_some(())?;
        self.layers_for_range(&range)
            .into_iter()
            .max_by_key(|layer| (layer.depth, std::cmp::Reverse(layer.range.len())))
            .map(|layer| layer.language.name())
    }

    /// 返回严格包围当前范围的最小语法节点，对齐 Zed `syntax_ancestor` 的选择扩展语义。
    pub fn ancestor_range(&self, range: Range<usize>, text: &Snapshot) -> Option<Range<usize>> {
        self.can_query(&range, text).then_some(())?;
        let mut best: Option<(Range<usize>, u32)> = None;
        for layer in self.layers_for_range(&range) {
            let Some(mut node) = layer
                .tree
                .root_node()
                .descendant_for_byte_range(range.start, range.end)
            else {
                continue;
            };
            loop {
                let candidate = node.byte_range();
                if encloses(&candidate, &range) && candidate.len() > range.len() {
                    let replace = best.as_ref().is_none_or(|(current, depth)| {
                        candidate.len() < current.len()
                            || (candidate.len() == current.len() && layer.depth > *depth)
                    });
                    if replace {
                        best = Some((candidate, layer.depth));
                    }
                    break;
                }
                let Some(parent) = node.parent() else {
                    break;
                };
                node = parent;
            }
        }
        best.map(|(range, _)| range)
    }

    pub fn bracket_pairs(&self, range: Range<usize>, text: &Snapshot) -> Vec<BracketPair> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }
        let mut pairs = Vec::new();
        for layer in self.layers_for_range(&range) {
            let Some(query) = layer.language.brackets() else {
                continue;
            };
            let names = query.capture_names();
            let mut cursor = QueryCursorHandle::new();
            cursor.set_byte_range(range.clone());
            let mut matches =
                cursor.matches(query, layer.tree.root_node(), SnapshotTextProvider(text));
            while let Some(query_match) = matches.next() {
                let mut open = None;
                let mut close = None;
                for capture in query_match.captures {
                    match names.get(capture.index as usize).copied() {
                        Some("open") => open = Some(capture.node.byte_range()),
                        Some("close") => close = Some(capture.node.byte_range()),
                        _ => {}
                    }
                }
                if let (Some(open), Some(close)) = (open, close) {
                    pairs.push(BracketPair { open, close });
                }
            }
        }
        pairs.sort_unstable_by_key(|pair| (pair.open.start, pair.close.end));
        pairs
    }

    pub fn outline_items(&self, range: Range<usize>, text: &Snapshot) -> Vec<OutlineItem> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }
        let mut items = Vec::new();
        for layer in self.layers_for_range(&range) {
            let Some(query) = layer.language.outline() else {
                continue;
            };
            let names = query.capture_names();
            let mut cursor = QueryCursorHandle::new();
            cursor.set_byte_range(range.clone());
            let mut matches =
                cursor.matches(query, layer.tree.root_node(), SnapshotTextProvider(text));
            while let Some(query_match) = matches.next() {
                let mut item = None;
                let mut names_ranges = Vec::new();
                let mut contexts = Vec::new();
                let mut open = None;
                let mut close = None;
                for capture in query_match.captures {
                    let capture_range = capture.node.byte_range();
                    match names.get(capture.index as usize).copied() {
                        Some("item") => item = Some(capture_range),
                        Some("name") => names_ranges.push(capture_range),
                        Some("context") => contexts.push(capture_range),
                        Some("open") => open = Some(capture_range.end),
                        Some("close") => close = Some(capture_range.start),
                        _ => {}
                    }
                }
                let Some(item) = item else { continue };
                items.push(OutlineItem {
                    range: item,
                    name_ranges: names_ranges,
                    context_ranges: contexts,
                    body_range: open
                        .zip(close)
                        .and_then(|(start, end)| (start <= end).then_some(start..end)),
                });
            }
        }
        items.sort_unstable_by_key(|item| (item.range.start, item.range.end));
        items
    }

    pub fn indent_ranges(&self, range: Range<usize>, text: &Snapshot) -> Vec<IndentRange> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }
        let mut ranges = Vec::new();
        for layer in self.layers_for_range(&range) {
            let Some(query) = layer.language.indents() else {
                continue;
            };
            let names = query.capture_names();
            let mut cursor = QueryCursorHandle::new();
            cursor.set_byte_range(range.clone());
            let mut matches =
                cursor.matches(query, layer.tree.root_node(), SnapshotTextProvider(text));
            while let Some(query_match) = matches.next() {
                let mut indent = None;
                let mut end = None;
                for capture in query_match.captures {
                    match names.get(capture.index as usize).copied() {
                        Some("indent") => indent = Some(capture.node.byte_range()),
                        Some("end") => end = Some(capture.node.byte_range()),
                        _ => {}
                    }
                }
                if let Some(range) = indent {
                    ranges.push(IndentRange { range, end });
                }
            }
        }
        ranges.sort_unstable_by_key(|range| (range.range.start, range.range.end));
        ranges
    }

    pub fn text_object_ranges(&self, range: Range<usize>, text: &Snapshot) -> Vec<TextObjectRange> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }
        let mut ranges = Vec::new();
        for layer in self.layers_for_range(&range) {
            let Some(query) = layer.language.text_objects() else {
                continue;
            };
            let names = query.capture_names();
            let mut cursor = QueryCursorHandle::new();
            cursor.set_byte_range(range.clone());
            let mut captures =
                cursor.captures(query, layer.tree.root_node(), SnapshotTextProvider(text));
            while let Some((query_match, capture_index)) = captures.next() {
                let capture = query_match.captures[*capture_index];
                let Some(kind) = names.get(capture.index as usize) else {
                    continue;
                };
                ranges.push(TextObjectRange {
                    kind: Arc::from(*kind),
                    range: capture.node.byte_range(),
                });
            }
        }
        ranges.sort_unstable_by_key(|range| (range.range.start, range.range.end));
        ranges
    }

    fn can_query(&self, range: &Range<usize>, text: &Snapshot) -> bool {
        text.version() == self.version
            && range.start <= range.end
            && range.end <= text.len_bytes().get()
    }

    fn layers_for_range<'a>(&'a self, range: &Range<usize>) -> Vec<SyntaxLayerRef<'a>> {
        let mut layers = Vec::new();
        if let (Some(language), Some(tree)) = (&self.language, &self.tree) {
            layers.push(SyntaxLayerRef {
                language,
                tree,
                range: tree.root_node().byte_range(),
                depth: 0,
            });
        }
        layers.extend(
            self.injections
                .iter()
                .filter(|layer| range_touches(&layer.range, range))
                .map(|layer| SyntaxLayerRef {
                    language: &layer.language,
                    tree: &layer.tree,
                    range: layer.range.clone(),
                    depth: layer.depth,
                }),
        );
        layers
    }

    /// 在调用线程完成真正的 tree-sitter 增量解析。
    /// 调用方应把该方法放到后台执行，再通过 `SyntaxMap::did_parse` 安装结果。
    pub fn reparse(mut self, snapshot: &Snapshot) -> Self {
        let Some(language) = self.language.as_ref() else {
            self.tree = None;
            self.injections.clear();
            self.version = snapshot.version();
            return self;
        };
        self.tree = parse_tree(language, snapshot, self.tree.as_ref(), None);
        let old_injections = std::mem::take(&mut self.injections);
        if let Some(tree) = self.tree.as_ref() {
            collect_injections(
                language,
                tree,
                snapshot,
                1,
                &old_injections,
                &mut self.injections,
            );
        }
        self.version = snapshot.version();
        self.rebuild_capture_table();
        self
    }

    /// 当前快照的 capture 名字全局表（capture index -> 名字）。
    ///
    /// 渲染侧用它对每个 capture index 做一次数组索引取样式，不再逐 run 做字符串回退查找。
    pub fn capture_names(&self) -> Arc<[Arc<str>]> {
        Arc::clone(&self.capture_names)
    }

    /// 重建跨语言 capture 名字全局表：主语言与注入语言的名字合并去重，
    /// 使 `HighlightSpan::capture` 在快照内跨语言唯一。
    fn rebuild_capture_table(&mut self) {
        let mut names: Vec<Arc<str>> = Vec::new();
        let mut index_by_name: HashMap<Arc<str>, u32> = HashMap::new();
        let mut add_language = |language: &Language| {
            for name in language.capture_names() {
                if !index_by_name.contains_key(name) {
                    index_by_name.insert(Arc::clone(name), names.len() as u32);
                    names.push(Arc::clone(name));
                }
            }
        };
        if let Some(language) = &self.language {
            add_language(language);
        }
        for layer in &self.injections {
            add_language(&layer.language);
        }
        self.capture_names = Arc::from(names);
        self.capture_index_by_name = Arc::new(index_by_name);
    }

    /// 查询指定字节范围，并像 Zed 的 BufferChunks 一样让更内层、后出现的 capture 覆盖外层。
    pub fn highlights(&self, range: Range<usize>, text: &Snapshot) -> Vec<HighlightSpan> {
        if text.version() != self.version || range.start >= range.end {
            return Vec::new();
        }
        let (Some(language), Some(tree)) = (&self.language, &self.tree) else {
            return Vec::new();
        };

        let mut events = Vec::new();
        let mut ordinal = 0usize;
        collect_highlight_events(
            language,
            tree,
            range.clone(),
            text,
            self,
            &mut ordinal,
            &mut events,
        );
        let mut injections: Vec<_> = self
            .injections
            .iter()
            .filter(|layer| ranges_overlap(&layer.range, &range))
            .collect();
        injections.sort_unstable_by_key(|layer| layer.depth);
        for layer in injections {
            let layer_range = layer.range.start.max(range.start)..layer.range.end.min(range.end);
            collect_highlight_events(
                &layer.language,
                &layer.tree,
                layer_range,
                text,
                self,
                &mut ordinal,
                &mut events,
            );
        }
        events.sort_unstable_by_key(HighlightEvent::sort_key);

        let mut active = BTreeMap::new();
        let mut spans: Vec<HighlightSpan> = Vec::new();
        let mut index = 0;
        while index < events.len() {
            let offset = events[index].offset();
            while index < events.len() && events[index].offset() == offset {
                match &events[index] {
                    HighlightEvent::Start {
                        ordinal, capture, ..
                    } => {
                        active.insert(*ordinal, *capture);
                    }
                    HighlightEvent::End { ordinal, .. } => {
                        active.remove(ordinal);
                    }
                }
                index += 1;
            }
            let Some(next_offset) = events.get(index).map(HighlightEvent::offset) else {
                break;
            };
            let Some((_, capture)) = active.last_key_value() else {
                continue;
            };
            if offset < next_offset {
                if let Some(last) = spans.last_mut()
                    && last.range.end == offset
                    && last.capture == *capture
                {
                    last.range.end = next_offset;
                } else {
                    spans.push(HighlightSpan {
                        range: offset..next_offset,
                        capture: *capture,
                    });
                }
            }
        }
        spans
    }
}

struct SyntaxLayerRef<'a> {
    language: &'a Language,
    tree: &'a tree_sitter::Tree,
    range: Range<usize>,
    depth: u32,
}

#[derive(Clone)]
enum HighlightEvent {
    Start {
        offset: usize,
        ordinal: usize,
        /// 快照全局 capture 名字表的索引。
        capture: u32,
    },
    End {
        offset: usize,
        ordinal: usize,
    },
}

impl HighlightEvent {
    fn offset(&self) -> usize {
        match self {
            Self::Start { offset, .. } | Self::End { offset, .. } => *offset,
        }
    }

    fn sort_key(&self) -> (usize, bool, usize) {
        match self {
            Self::End {
                offset, ordinal, ..
            } => (*offset, false, *ordinal),
            Self::Start {
                offset, ordinal, ..
            } => (*offset, true, *ordinal),
        }
    }
}

fn collect_highlight_events(
    language: &Language,
    tree: &tree_sitter::Tree,
    range: Range<usize>,
    text: &Snapshot,
    snapshot: &SyntaxSnapshot,
    ordinal: &mut usize,
    events: &mut Vec<HighlightEvent>,
) {
    if range.start >= range.end {
        return;
    }
    let mut cursor = QueryCursorHandle::new();
    cursor.set_byte_range(range.clone());
    let mut captures = cursor.captures(
        language.highlights(),
        tree.root_node(),
        SnapshotTextProvider(text),
    );
    while let Some((query_match, capture_index)) = captures.next() {
        let capture = query_match.captures[*capture_index];
        let capture_range = capture.node.byte_range();
        let start = capture_range.start.max(range.start);
        let end = capture_range.end.min(range.end);
        let Some(capture_name) = language.capture_name(capture.index) else {
            continue;
        };
        // 语言内 index → 快照全局 index：注入层的 capture 也要能由渲染侧统一查表。
        let Some(&global_capture) = snapshot.capture_index_by_name.get(&capture_name) else {
            continue;
        };
        if start < end {
            events.push(HighlightEvent::Start {
                offset: start,
                ordinal: *ordinal,
                capture: global_capture,
            });
            events.push(HighlightEvent::End {
                offset: end,
                ordinal: *ordinal,
            });
            *ordinal += 1;
        }
    }
}

fn parse_tree(
    language: &Language,
    snapshot: &Snapshot,
    old_tree: Option<&tree_sitter::Tree>,
    included_range: Option<Range<usize>>,
) -> Option<tree_sitter::Tree> {
    let mut parser = Parser::new();
    parser.set_language(language.grammar()).ok()?;
    if let Some(range) = included_range {
        let start = point_at(snapshot, ByteOffset::new(range.start)).ok()?;
        let end = point_at(snapshot, ByteOffset::new(range.end)).ok()?;
        parser
            .set_included_ranges(&[tree_sitter::Range {
                start_byte: range.start,
                end_byte: range.end,
                start_point: start,
                end_point: end,
            }])
            .ok()?;
    }
    parser.parse_with_options(
        &mut |offset, _| chunk_from(snapshot, offset),
        old_tree,
        None,
    )
}

fn collect_injections(
    parent_language: &Language,
    parent_tree: &tree_sitter::Tree,
    snapshot: &Snapshot,
    depth: u32,
    old_layers: &[SyntaxLayer],
    layers: &mut Vec<SyntaxLayer>,
) {
    const MAX_INJECTION_DEPTH: u32 = 8;
    if depth > MAX_INJECTION_DEPTH {
        return;
    }
    let Some(query) = parent_language.injections() else {
        return;
    };

    let capture_names = query.capture_names();
    let mut cursor = QueryCursorHandle::new();
    let mut matches = cursor.matches(
        query,
        parent_tree.root_node(),
        SnapshotTextProvider(snapshot),
    );
    while let Some(query_match) = matches.next() {
        let mut language_name = query
            .property_settings(query_match.pattern_index)
            .iter()
            .find(|property| property.key.as_ref() == "injection.language")
            .and_then(|property| property.value.as_deref())
            .map(str::to_owned);
        let mut content_ranges = Vec::new();
        for capture in query_match.captures {
            match capture_names.get(capture.index as usize).copied() {
                Some("injection.content") => content_ranges.push(capture.node.byte_range()),
                Some("injection.language") => {
                    language_name = node_text(snapshot, capture.node.byte_range());
                }
                _ => {}
            }
        }
        let Some(language_name) = language_name else {
            continue;
        };
        let Some(language) = language_for_injection(&language_name) else {
            continue;
        };
        for range in content_ranges {
            if range.start >= range.end
                || layers.iter().any(|layer| {
                    layer.depth == depth
                        && layer.language.name() == language.name()
                        && layer.range == range
                })
            {
                continue;
            }
            let old_tree = old_layers
                .iter()
                .find(|layer| {
                    layer.depth == depth
                        && layer.language.name() == language.name()
                        && layer.range == range
                })
                .map(|layer| &layer.tree);
            let Some(tree) = parse_tree(&language, snapshot, old_tree, Some(range.clone())) else {
                continue;
            };
            collect_injections(&language, &tree, snapshot, depth + 1, old_layers, layers);
            layers.push(SyntaxLayer {
                language: language.clone(),
                tree,
                range,
                depth,
            });
        }
    }
}

fn node_text(snapshot: &Snapshot, range: Range<usize>) -> Option<String> {
    snapshot
        .slice_byte_range(ByteOffset::new(range.start), ByteOffset::new(range.end))
        .ok()
        .map(|text| text.as_str().trim().to_owned())
}

fn edit_tree(
    tree: &mut tree_sitter::Tree,
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    changes: &TextChangeBatch,
) -> bool {
    for edit in changes.patch().edits().iter().rev() {
        let old = edit.old_range();
        let new = edit.new_range();
        let (Ok(start_position), Ok(old_end_position), Ok(inserted)) = (
            point_at(old_snapshot, old.start()),
            point_at(old_snapshot, old.end()),
            new_snapshot.slice_text(new),
        ) else {
            return false;
        };
        tree.edit(&InputEdit {
            start_byte: old.start().get(),
            old_end_byte: old.end().get(),
            new_end_byte: old.start().get() + new.len(),
            start_position,
            old_end_position,
            new_end_position: advance_point(start_position, inserted.as_str()),
        });
    }
    true
}

fn map_range_through_changes(range: Range<usize>, changes: &TextChangeBatch) -> Range<usize> {
    map_offset(range.start, true, changes)..map_offset(range.end, false, changes)
}

fn map_offset(offset: usize, before: bool, changes: &TextChangeBatch) -> usize {
    let mut delta = 0isize;
    for edit in changes.patch().edits() {
        let old = edit.old_range();
        let new = edit.new_range();
        if offset < old.start().get() || (before && offset == old.start().get()) {
            break;
        }
        if offset <= old.end().get() {
            return if before {
                new.start().get()
            } else {
                new.end().get()
            };
        }
        delta += new.len() as isize - old.len() as isize;
    }
    offset.saturating_add_signed(delta)
}

fn ranges_overlap(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start < right.end && right.start < left.end
}

fn range_touches(layer: &Range<usize>, query: &Range<usize>) -> bool {
    if query.is_empty() {
        layer.start <= query.start && query.start < layer.end
    } else {
        ranges_overlap(layer, query)
    }
}

fn encloses(outer: &Range<usize>, inner: &Range<usize>) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

struct QueryCursorHandle(Option<QueryCursor>);

impl QueryCursorHandle {
    fn new() -> Self {
        let mut cursor = query_cursor_pool()
            .lock()
            .expect("QueryCursor 池不应在持锁期间 panic")
            .pop()
            .unwrap_or_else(QueryCursor::new);
        cursor.set_match_limit(64);
        Self(Some(cursor))
    }
}

impl Deref for QueryCursorHandle {
    type Target = QueryCursor;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("QueryCursorHandle 持有有效 cursor")
    }
}

impl DerefMut for QueryCursorHandle {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("QueryCursorHandle 持有有效 cursor")
    }
}

impl Drop for QueryCursorHandle {
    fn drop(&mut self) {
        if let Some(cursor) = self.0.take() {
            query_cursor_pool()
                .lock()
                .expect("QueryCursor 池不应在持锁期间 panic")
                .push(cursor);
        }
    }
}

fn query_cursor_pool() -> &'static Mutex<Vec<QueryCursor>> {
    static QUERY_CURSORS: OnceLock<Mutex<Vec<QueryCursor>>> = OnceLock::new();
    QUERY_CURSORS.get_or_init(|| Mutex::new(Vec::new()))
}

struct SnapshotTextProvider<'a>(&'a Snapshot);

impl<'a> tree_sitter::TextProvider<&'a [u8]> for SnapshotTextProvider<'a> {
    type I = SnapshotByteChunks<'a>;

    fn text(&mut self, node: tree_sitter::Node) -> Self::I {
        SnapshotByteChunks {
            snapshot: self.0,
            next: node.start_byte(),
            end: node.end_byte(),
        }
    }
}

struct SnapshotByteChunks<'a> {
    snapshot: &'a Snapshot,
    next: usize,
    end: usize,
}

impl<'a> Iterator for SnapshotByteChunks<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.end {
            return None;
        }
        let (chunk, chunk_start) = self
            .snapshot
            .chunk_at_byte(ByteOffset::new(self.next))
            .ok()?;
        let local_start = self.next - chunk_start.get();
        let local_end = (self.end - chunk_start.get()).min(chunk.len());
        let bytes = &chunk.as_bytes()[local_start..local_end];
        self.next += bytes.len();
        Some(bytes)
    }
}

fn chunk_from(snapshot: &Snapshot, offset: usize) -> &[u8] {
    if offset >= snapshot.len_bytes().get() {
        return &[];
    }
    let Ok((chunk, chunk_start)) = snapshot.chunk_at_byte(ByteOffset::new(offset)) else {
        return &[];
    };
    &chunk.as_bytes()[offset - chunk_start.get()..]
}

fn point_at(snapshot: &Snapshot, offset: ByteOffset) -> zcv_engine::EngineResult<Point> {
    let (line, column) = snapshot.byte_to_point(offset)?;
    Ok(Point::new(line.get(), column))
}

fn advance_point(start: Point, text: &str) -> Point {
    let mut rows = 0;
    let mut last_line_bytes = 0;
    for part in text.split_inclusive('\n') {
        if part.ends_with('\n') {
            rows += 1;
            last_line_bytes = 0;
        } else {
            last_line_bytes = part.len();
        }
    }
    if rows == 0 {
        Point::new(start.row, start.column + text.len())
    } else {
        Point::new(start.row + rows, last_line_bytes)
    }
}

#[cfg(test)]
mod tests {
    use zcv_engine::{Buffer, BufferConfig, Edit, TextRange, Transaction};

    use super::*;

    fn rust_buffer(text: &str) -> (Buffer, SyntaxMap) {
        let buffer = Buffer::from_text(text.to_owned(), BufferConfig::default()).unwrap();
        let snapshot = buffer.snapshot();
        let mut syntax = SyntaxMap::new(&snapshot);
        syntax.set_language_for_file(Path::new("main.rs"), &snapshot);
        let parsed = syntax.snapshot().reparse(&snapshot);
        assert!(syntax.did_parse(parsed));
        (buffer, syntax)
    }

    fn parsed_syntax(path: &str, text: &str) -> (Buffer, SyntaxMap) {
        let buffer = Buffer::from_text(text.to_owned(), BufferConfig::default()).unwrap();
        let snapshot = buffer.snapshot();
        let mut syntax = SyntaxMap::new(&snapshot);
        syntax.set_language_for_file(Path::new(path), &snapshot);
        let parsed = syntax.snapshot().reparse(&snapshot);
        assert!(syntax.did_parse(parsed));
        (buffer, syntax)
    }

    #[test]
    fn highlights_rust_captures_in_unicode_text() {
        let (buffer, syntax) = rust_buffer("fn 问候() { let 文本 = \"你好\"; }\n");
        let snapshot = buffer.snapshot();
        let syntax_snapshot = syntax.snapshot();
        let names = syntax_snapshot.capture_names();
        let spans = syntax_snapshot.highlights(0..snapshot.len_bytes().get(), &snapshot);

        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "keyword")
        );
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "function")
        );
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "string")
        );
        assert!(
            spans
                .iter()
                .all(|span| span.range.end <= snapshot.len_bytes().get())
        );
    }

    #[test]
    fn rust_syntax_snapshot_exposes_zed_structure_queries() {
        let source = "struct Demo { value: i32 }\nfn main() { let x = (1 + 2); }\n";
        let (buffer, syntax) = rust_buffer(source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let full = 0..snapshot.len_bytes().get();

        let brackets = syntax.bracket_pairs(full.clone(), &snapshot);
        assert!(brackets.iter().any(|pair| {
            &source[pair.open.clone()] == "(" && &source[pair.close.clone()] == ")"
        }));

        let outline = syntax.outline_items(full.clone(), &snapshot);
        let names: Vec<_> = outline
            .iter()
            .flat_map(|item| item.name_ranges.iter())
            .map(|range| &source[range.clone()])
            .collect();
        assert!(names.contains(&"Demo"));
        assert!(names.contains(&"main"));
        assert!(outline.iter().any(|item| item.body_range.is_some()));

        assert!(!syntax.indent_ranges(full.clone(), &snapshot).is_empty());
        assert!(
            syntax
                .text_object_ranges(full, &snapshot)
                .iter()
                .any(|range| range.kind.as_ref() == "function.around")
        );
    }

    #[test]
    fn syntax_ancestor_and_injected_language_use_the_smallest_layer() {
        let source = "fn main() { let value = 1; }\n";
        let (buffer, syntax) = rust_buffer(source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let caret = source.find("value").unwrap();
        let identifier = syntax
            .ancestor_range(caret..caret, &snapshot)
            .expect("光标应扩展到 identifier");
        assert_eq!(&source[identifier.clone()], "value");
        let parent = syntax
            .ancestor_range(identifier, &snapshot)
            .expect("identifier 应继续扩展到父语法节点");
        assert!(parent.len() > "value".len());

        let html = "<style>.item { color: red; }</style>";
        let (buffer, syntax) = parsed_syntax("index.html", html);
        let snapshot = buffer.snapshot();
        let offset = html.find("color").unwrap();
        assert_eq!(
            syntax.snapshot().language_at(offset, &snapshot),
            Some("CSS")
        );
    }

    #[test]
    fn incrementally_reparses_after_edit() {
        let (mut buffer, mut syntax) = rust_buffer("fn main() { let value = 1; }\n");
        let subscription = buffer.subscribe();
        let old_snapshot = buffer.snapshot();
        let start = old_snapshot
            .slice_byte_range(ByteOffset::ZERO, old_snapshot.len_bytes())
            .unwrap()
            .as_str()
            .find('1')
            .unwrap();
        buffer.insert(ByteOffset::new(start), "\"文本\"").unwrap();
        let new_snapshot = buffer.snapshot();
        syntax.interpolate(&old_snapshot, &new_snapshot, &subscription.consume());
        let parsed = syntax.snapshot().reparse(&new_snapshot);
        assert!(syntax.did_parse(parsed));

        let syntax_snapshot = syntax.snapshot();
        let names = syntax_snapshot.capture_names();
        let spans = syntax_snapshot.highlights(0..new_snapshot.len_bytes().get(), &new_snapshot);
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "string")
        );
        assert_eq!(syntax.snapshot().version(), new_snapshot.version());
    }

    #[test]
    fn incrementally_reparses_multiple_unicode_edits() {
        let (mut buffer, mut syntax) = rust_buffer("fn main() { let x = 1; let y = 2; }\n");
        let subscription = buffer.subscribe();
        let old_snapshot = buffer.snapshot();
        let source = old_snapshot
            .slice_byte_range(ByteOffset::ZERO, old_snapshot.len_bytes())
            .unwrap();
        let first = source.as_str().find('1').unwrap();
        let second = source.as_str().find('2').unwrap();
        let transaction = Transaction::from_edits(
            buffer.version(),
            vec![
                Edit::replace(
                    TextRange::new(ByteOffset::new(first), ByteOffset::new(first + 1)).unwrap(),
                    "\"一\"",
                ),
                Edit::replace(
                    TextRange::new(ByteOffset::new(second), ByteOffset::new(second + 1)).unwrap(),
                    "\"二\"",
                ),
            ],
        )
        .unwrap();
        buffer.apply_transaction(transaction).unwrap();
        let new_snapshot = buffer.snapshot();
        syntax.interpolate(&old_snapshot, &new_snapshot, &subscription.consume());
        let parsed = syntax.snapshot().reparse(&new_snapshot);
        assert!(syntax.did_parse(parsed));

        let syntax_snapshot = syntax.snapshot();
        let names = syntax_snapshot.capture_names();
        let string_count = syntax_snapshot
            .highlights(0..new_snapshot.len_bytes().get(), &new_snapshot)
            .iter()
            .filter(|span| names[span.capture as usize].as_ref() == "string")
            .count();
        assert_eq!(string_count, 2);
    }

    #[test]
    fn markdown_inline_layer_overrides_block_highlights() {
        let (buffer, syntax) = parsed_syntax("README.md", "普通 *强调* 和 **加粗**\n");
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        assert!(
            syntax
                .injections
                .iter()
                .any(|layer| layer.language.name() == "Markdown Inline")
        );
        let names = syntax.capture_names();
        let spans = syntax.highlights(0..snapshot.len_bytes().get(), &snapshot);
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "text.emphasis")
        );
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "text.strong")
        );
    }

    #[test]
    fn html_injects_css_and_javascript_layers() {
        let source = "<style>.item { color: red; }</style><script>let value = 1;</script>";
        let (buffer, syntax) = parsed_syntax("index.html", source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        assert!(
            syntax
                .injections
                .iter()
                .any(|layer| layer.language.name() == "CSS")
        );
        assert!(
            syntax
                .injections
                .iter()
                .any(|layer| layer.language.name() == "JavaScript")
        );
        let names = syntax.capture_names();
        let spans = syntax.highlights(0..snapshot.len_bytes().get(), &snapshot);
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "property")
        );
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "keyword")
        );
    }

    #[test]
    fn stale_parse_result_cannot_replace_interpolated_tree() {
        let (mut buffer, mut syntax) = rust_buffer("fn main() {}\n");
        let stale_parse = syntax.snapshot();
        let subscription = buffer.subscribe();
        let old_snapshot = buffer.snapshot();
        buffer.insert(ByteOffset::new(3), "async ").unwrap();
        let new_snapshot = buffer.snapshot();
        syntax.interpolate(&old_snapshot, &new_snapshot, &subscription.consume());

        let stale = stale_parse.reparse(&old_snapshot);
        assert!(!syntax.did_parse(stale));
        assert_eq!(syntax.snapshot().version(), new_snapshot.version());
    }
}
