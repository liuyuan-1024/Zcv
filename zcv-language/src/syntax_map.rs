use std::collections::{HashMap, HashSet};
use std::ops::{ControlFlow, Range};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::thread;

use tree_sitter::StreamingIterator;
use zcv_text::{BufferVersion, Snapshot, TextChangeBatch};

use crate::Language;
use crate::registry::{language_for_file, language_for_injection};
use crate::tree_sitter_utils::{
    IncrementalParser, PARSE_TIME_SLICE, ParseCancellation, QueryCursorHandle,
    SnapshotTextProvider, drop_offloaded, edit_tree, encloses, map_range_through_changes,
    node_text, parse_tree, ranges_overlap,
};

/// 可增量更新的语法状态。
///
/// `parsed_version` 表示 Tree 真正完成解析的版本；
/// `interpolated_version` 表示旧树已经通过 `InputEdit` 推进到的文本版本。
/// 两者分离后，前台可以立即使用坐标正确的旧树，真正的增量解析则交给后台完成。
pub(crate) struct SyntaxMap {
    language: Option<Arc<Language>>,
    state: Arc<SyntaxState>,
    parsed_version: BufferVersion,
    interpolated_version: BufferVersion,
}

#[derive(Clone, Debug, Default)]
struct SyntaxState {
    tree: Option<tree_sitter::Tree>,
    injections: Vec<SyntaxLayer>,
    /// 最近一次解析安装的 capture 全局表（见 `SyntaxSnapshot::rebuild_capture_table`）。
    capture_names: Arc<[Arc<str>]>,
    capture_index_by_language: HashMap<&'static str, Arc<[u32]>>,
}

impl SyntaxState {
    fn has_trees(&self) -> bool {
        self.tree.is_some() || !self.injections.is_empty()
    }
}

/// 与一个 Buffer 版本绑定的不可变语法快照。
///
/// 语法树、注入层和 capture 表共享同一份不可变负载；
/// 只有插值或解析真正修改语法状态时才通过 `Arc::make_mut` 复制。
#[derive(Clone, Debug)]
pub struct SyntaxSnapshot {
    pub(crate) language: Option<Arc<Language>>,
    state: Arc<SyntaxState>,
    pub(crate) version: BufferVersion,
}

impl Drop for SyntaxSnapshot {
    fn drop(&mut self) {
        offload_state_if_last(std::mem::replace(&mut self.state, empty_syntax_state()));
    }
}

impl Drop for SyntaxMap {
    fn drop(&mut self) {
        offload_state_if_last(std::mem::replace(&mut self.state, empty_syntax_state()));
    }
}

fn offload_state_if_last(state: Arc<SyntaxState>) {
    if state.has_trees() && Arc::strong_count(&state) == 1 {
        drop_offloaded(state);
    }
}

fn empty_syntax_state() -> Arc<SyntaxState> {
    static EMPTY: OnceLock<Arc<SyntaxState>> = OnceLock::new();
    Arc::clone(EMPTY.get_or_init(|| Arc::new(SyntaxState::default())))
}

#[derive(Clone, Debug)]
pub(crate) struct SyntaxLayer {
    pub(crate) language: Arc<Language>,
    pub(crate) tree: tree_sitter::Tree,
    pub(crate) range: Range<usize>,
    pub(crate) depth: u32,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct InjectionKey {
    depth: u32,
    language: &'static str,
    start: usize,
    end: usize,
}

impl InjectionKey {
    fn new(depth: u32, language: &Language, range: &Range<usize>) -> Self {
        Self {
            depth,
            language: language.name(),
            start: range.start,
            end: range.end,
        }
    }
}

impl SyntaxMap {
    pub(crate) fn language(&self) -> Option<&Language> {
        self.language.as_deref()
    }

    pub(crate) fn new(snapshot: &Snapshot) -> Self {
        Self {
            language: None,
            state: empty_syntax_state(),
            parsed_version: snapshot.version(),
            interpolated_version: snapshot.version(),
        }
    }

    pub(crate) fn set_language_for_file(
        &mut self,
        path: &Path,
        first_line: Option<&str>,
        snapshot: &Snapshot,
    ) -> bool {
        self.set_language(language_for_file(path, first_line), snapshot)
    }

    pub(crate) fn set_language(
        &mut self,
        language: Option<Arc<Language>>,
        snapshot: &Snapshot,
    ) -> bool {
        let unchanged = match (&self.language, &language) {
            (Some(current), Some(next)) => Arc::ptr_eq(current, next),
            (None, None) => true,
            _ => false,
        };
        if unchanged {
            return false;
        }
        self.language = language;
        offload_state_if_last(std::mem::replace(&mut self.state, empty_syntax_state()));
        self.parsed_version = snapshot.version();
        self.interpolated_version = snapshot.version();
        true
    }

    /// 只把旧树推进到新坐标，不在调用线程执行解析。
    pub(crate) fn interpolate(
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

        let state = Arc::make_mut(&mut self.state);
        let mut tree = state.tree.take();
        if can_increment {
            if tree
                .as_mut()
                .is_some_and(|tree| !edit_tree(tree, old_snapshot, new_snapshot, changes))
                && let Some(old_tree) = tree.take()
            {
                drop_offloaded(old_tree);
            }
            let mut invalid_layers = Vec::new();
            let old_layers = std::mem::take(&mut state.injections);
            for mut layer in old_layers {
                if edit_tree(&mut layer.tree, old_snapshot, new_snapshot, changes) {
                    layer.range = map_range_through_changes(layer.range, changes);
                    if layer.range.start < layer.range.end {
                        state.injections.push(layer);
                        continue;
                    }
                }
                invalid_layers.push(layer);
            }
            if !invalid_layers.is_empty() {
                drop_offloaded(invalid_layers);
            }
        } else {
            let old_layers = std::mem::take(&mut state.injections);
            if tree.is_some() || !old_layers.is_empty() {
                drop_offloaded((tree.take(), old_layers));
            }
        }

        state.tree = tree;
        self.interpolated_version = new_snapshot.version();
    }

    pub(crate) fn snapshot(&self) -> SyntaxSnapshot {
        SyntaxSnapshot {
            language: self.language.clone(),
            state: Arc::clone(&self.state),
            version: self.interpolated_version,
        }
    }

    pub(crate) fn did_parse(&mut self, mut parsed: SyntaxSnapshot) -> bool {
        let same_language = match (&parsed.language, &self.language) {
            (Some(parsed), Some(current)) => Arc::ptr_eq(parsed, current),
            (None, None) => true,
            _ => false,
        };
        if parsed.version != self.interpolated_version || !same_language {
            return false;
        }
        let parsed_state = std::mem::replace(&mut parsed.state, empty_syntax_state());
        let old_state = std::mem::replace(&mut self.state, parsed_state);
        offload_state_if_last(old_state);
        self.parsed_version = parsed.version;
        true
    }
}

impl SyntaxSnapshot {
    /// 空语法快照（无语言、无树）：语言匹配前或未安装语法时的占位，查询一律返回空。
    pub fn empty(version: BufferVersion) -> Self {
        Self {
            language: None,
            state: empty_syntax_state(),
            version,
        }
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    pub fn has_language(&self) -> bool {
        self.language.is_some()
    }

    /// 返回严格包围当前范围的最小语法节点，用于选择扩展。
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

    pub(crate) fn can_query(&self, range: &Range<usize>, text: &Snapshot) -> bool {
        text.version() == self.version
            && range.start <= range.end
            && range.end <= text.len_bytes().get()
    }

    /// 返回与范围相交的语法层（主语言层 + 注入层），零堆分配。
    ///
    /// 注入层按 (深度, 起点) 有序且同深互不相交（注入内容节点在父树中要么嵌套要么不相交）：
    /// 每个深度用二分定位覆盖查询起点的层，再向前游走起点在查询终点之前的层——O(D log N + K)，不再扫描全部注入层。
    pub(crate) fn layers_for_range<'a>(
        &'a self,
        range: &'a Range<usize>,
    ) -> impl Iterator<Item = SyntaxLayerRef<'a>> + 'a {
        let main = match (&self.language, &self.state.tree) {
            (Some(language), Some(tree)) => Some(SyntaxLayerRef {
                language: language.as_ref(),
                tree,
                depth: 0,
            }),
            _ => None,
        };
        main.into_iter()
            .chain(LayersInRange::new(&self.state.injections, range))
    }

    /// 同步执行真正的 tree-sitter 增量解析。
    /// 调用方必须把该方法放到后台，再通过 `SyntaxMap::did_parse` 安装结果。
    ///
    /// `edits` 是本次编辑在新坐标下的字节区间：tree-sitter 的 `changed_ranges` 对等长替换（parser 直接复用旧叶子）不可见，必须用文本编辑区间兜底。
    /// 变化区间 = 编辑区间 ∪ 树变化区间，两者都不覆盖的区域注入层原样保留。
    pub(crate) fn reparse(
        mut self,
        snapshot: &Snapshot,
        edits: Option<&[Range<usize>]>,
        cancellation: &ParseCancellation,
    ) -> Option<Self> {
        if cancellation.is_cancelled() {
            return None;
        }
        let Some(language) = self.language.as_ref() else {
            self.state = empty_syntax_state();
            self.version = snapshot.version();
            return Some(self);
        };
        {
            let state = Arc::make_mut(&mut self.state);
            let old_tree = state.tree.take();
            // 主树解析按时间片进行：预算用尽中断后保留 parser 状态，下一片从断点恢复（每片 ~3ms，避免大文件解析长期独占后台线程）。
            let new_tree = if language.grammar().is_some() {
                let mut parser = IncrementalParser::new();
                loop {
                    let tree = parser.parse_slice(
                        language,
                        snapshot,
                        old_tree.as_ref(),
                        None,
                        cancellation,
                        PARSE_TIME_SLICE,
                    );
                    if cancellation.is_cancelled() {
                        return None;
                    }
                    if let Some(tree) = tree {
                        break Some(tree);
                    }
                    // 预算用尽：让出后台线程，下一片继续。
                    thread::yield_now();
                }
            } else {
                // 无语法树语言（纯文本兜底）：主树保持为空。
                None
            };
            if cancellation.is_cancelled() {
                return None;
            }
            // 变化区间：区间之外的注入与文本都未变，旧注入层原样保留，只在这些区间内重新收集注入。
            let changed = match (&old_tree, &new_tree) {
                (Some(old_tree), Some(new_tree)) => {
                    let mut ranges: Vec<Range<usize>> = old_tree
                        .changed_ranges(new_tree)
                        .map(|range| range.start_byte..range.end_byte)
                        .collect();
                    if let Some(edits) = edits {
                        ranges.extend(edits.iter().cloned());
                    }
                    merge_changed_ranges(ranges)
                }
                // 首次解析或插值被整体重置：无旧树可比，全文收集。
                _ => std::iter::once(0..snapshot.len_bytes().get()).collect(),
            };
            state.tree = new_tree;

            let old_injections = std::mem::take(&mut state.injections);
            // 范围与任何变化区间相交的旧层进入复用表（供增量解析）；其余原样保留。
            let mut seen = HashSet::new();
            let mut old_trees = HashMap::new();
            for layer in old_injections {
                let key = InjectionKey::new(layer.depth, &layer.language, &layer.range);
                if changed
                    .iter()
                    .any(|range| ranges_overlap(&layer.range, range))
                {
                    old_trees.insert(key, layer.tree);
                } else {
                    seen.insert(key);
                    state.injections.push(layer);
                }
            }

            let mut collected = Vec::new();
            if let Some(tree) = state.tree.as_ref() {
                let mut collector = InjectionCollector {
                    snapshot,
                    edits,
                    old_trees: &mut old_trees,
                    seen: &mut seen,
                    layers: &mut collected,
                    cancellation,
                };
                for range in &changed {
                    if !collector.collect(language, tree, range.clone(), 1) {
                        return None;
                    }
                }
            }

            // 保留层与重新收集层的同深重叠清理：变化区间边界可能命中同一注入（如围栏行编辑改了注入语言但内容范围未变），此时以新收集为准。
            let mut final_layers = Vec::with_capacity(state.injections.len() + collected.len());
            for layer in std::mem::take(&mut state.injections) {
                let replaced = collected.iter().any(|new| {
                    new.depth == layer.depth && ranges_overlap(&new.range, &layer.range)
                });
                if !replaced {
                    final_layers.push(layer);
                }
            }
            final_layers.extend(collected);
            // 按 (深度, 起点) 有序：每深度一段连续切片，供 layers_for_range 二分查询。
            final_layers
                .sort_unstable_by_key(|layer| (layer.depth, layer.range.start, layer.range.end));
            state.injections = final_layers;
        }
        self.version = snapshot.version();
        self.rebuild_capture_table();
        Some(self)
    }

    /// 当前快照的 capture 名字全局表（capture index -> 名字）。
    ///
    /// 渲染侧用它对每个 capture index 做一次数组索引取样式，不再逐 run 做字符串回退查找。
    pub fn capture_names(&self) -> Arc<[Arc<str>]> {
        Arc::clone(&self.state.capture_names)
    }

    /// 重建跨语言 capture 名字全局表：主语言与注入语言的名字合并去重，
    /// 使 `HighlightSpan::capture` 在快照内跨语言唯一；
    /// 同时构建每语言的局部 index -> 全局 index 映射，高亮收集时直接数组索引、零哈希查找。
    fn rebuild_capture_table(&mut self) {
        let mut names: Vec<Arc<str>> = Vec::new();
        let mut index_by_name: HashMap<Arc<str>, u32> = HashMap::new();
        let mut index_by_language: HashMap<&'static str, Arc<[u32]>> = HashMap::new();
        let mut add_language = |language: &Language| {
            let mut local = Vec::with_capacity(language.capture_names().len());
            for name in language.capture_names() {
                let global = if let Some(&index) = index_by_name.get(name) {
                    index
                } else {
                    let index = names.len() as u32;
                    index_by_name.insert(Arc::clone(name), index);
                    names.push(Arc::clone(name));
                    index
                };
                local.push(global);
            }
            index_by_language.insert(language.name(), Arc::from(local));
        };
        if let Some(language) = &self.language {
            add_language(language);
        }
        for layer in &self.state.injections {
            add_language(&layer.language);
        }
        let state = Arc::make_mut(&mut self.state);
        state.capture_names = Arc::from(names);
        state.capture_index_by_language = index_by_language;
    }

    /// 语言局部 capture index -> 快照全局 index 的映射（高亮收集用）。
    pub(crate) fn capture_index_table(&self, language: &Language) -> Option<&Arc<[u32]>> {
        self.state.capture_index_by_language.get(language.name())
    }

    pub(crate) fn injection_layers(&self) -> &[SyntaxLayer] {
        &self.state.injections
    }

    pub(crate) fn root_tree(&self) -> Option<&tree_sitter::Tree> {
        self.state.tree.as_ref()
    }
}

pub(crate) struct SyntaxLayerRef<'a> {
    pub(crate) language: &'a Language,
    pub(crate) tree: &'a tree_sitter::Tree,
    pub(crate) depth: u32,
}

/// 按 (深度, 起点) 有序的注入层上的范围查询迭代器。
///
/// 逐深度推进：二分定位起点 ≥ 查询起点处的首个层（`index`），先检查其前一层（可能覆盖查询起点的候选），再向前游走起点 < 查询终点的层。
/// 空查询按"点包含"语义处理（候选覆盖起点，或层恰从起点开始）。
struct LayersInRange<'a> {
    injections: &'a [SyntaxLayer],
    range: &'a Range<usize>,
    depth: u32,
    /// 当前深度二分得到的起点（第一个起点 ≥ 查询起点的层）。
    index: usize,
    /// 待检查的候选层（`index - 1`，可能覆盖查询起点）。
    candidate: Option<usize>,
}

impl<'a> LayersInRange<'a> {
    fn new(injections: &'a [SyntaxLayer], range: &'a Range<usize>) -> Self {
        let depth = 1;
        let index = injections
            .partition_point(|layer| (layer.depth, layer.range.start) < (depth, range.start));
        Self {
            injections,
            range,
            depth,
            index,
            candidate: index.checked_sub(1),
        }
    }

    fn layer_ref(&self, index: usize) -> SyntaxLayerRef<'a> {
        let layer = &self.injections[index];
        SyntaxLayerRef {
            language: layer.language.as_ref(),
            tree: &layer.tree,
            depth: layer.depth,
        }
    }
}

impl<'a> Iterator for LayersInRange<'a> {
    type Item = SyntaxLayerRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // 候选层：起点 < 查询起点且可能覆盖它（即使 index 已越界也必须检查）。
            if let Some(candidate) = self.candidate.take() {
                let layer = &self.injections[candidate];
                if layer.depth == self.depth && layer.range.end > self.range.start {
                    return Some(self.layer_ref(candidate));
                }
            }
            // 向前游走：同深且起点 < 查询终点（空查询时允许起点恰为查询起点）。
            if self.index < self.injections.len() {
                let layer = &self.injections[self.index];
                if layer.depth == self.depth {
                    let starts_before_end = layer.range.start < self.range.end
                        || (self.range.is_empty() && layer.range.start == self.range.start);
                    if starts_before_end {
                        self.index += 1;
                        return Some(self.layer_ref(self.index - 1));
                    }
                }
            }
            // 进入下一深度；所有深度处理完则结束。
            let last_depth = self.injections.last()?.depth;
            if self.depth >= last_depth {
                return None;
            }
            self.depth += 1;
            self.index = self.injections.partition_point(|layer| {
                (layer.depth, layer.range.start) < (self.depth, self.range.start)
            });
            self.candidate = self.index.checked_sub(1);
        }
    }
}

/// 按变化区间收集注入：查询限定在 `range` 内，旧树按注入键复用做增量解析，未变化的嵌套注入通过 `seen`（含全部保留层键）跳过，不重复收集。
struct InjectionCollector<'a> {
    snapshot: &'a Snapshot,
    /// 本次编辑的新坐标字节区间（等长替换等树变化不可见的信号，递归时按层范围裁剪）。
    edits: Option<&'a [Range<usize>]>,
    old_trees: &'a mut HashMap<InjectionKey, tree_sitter::Tree>,
    seen: &'a mut HashSet<InjectionKey>,
    layers: &'a mut Vec<SyntaxLayer>,
    cancellation: &'a ParseCancellation,
}

impl InjectionCollector<'_> {
    fn collect(
        &mut self,
        parent_language: &Language,
        parent_tree: &tree_sitter::Tree,
        range: Range<usize>,
        depth: u32,
    ) -> bool {
        const MAX_INJECTION_DEPTH: u32 = 8;
        if depth > MAX_INJECTION_DEPTH || self.cancellation.is_cancelled() {
            return !self.cancellation.is_cancelled();
        }
        let Some(query) = parent_language.injections() else {
            return true;
        };

        let capture_names = query.capture_names();
        let mut cursor = QueryCursorHandle::new();
        // 查询限定在变化区间：与区间相交的注入节点（含跨越边界的围栏块等）都会被命中，区间之外的注入不会进入收集路径。
        cursor.set_byte_range(range.clone());
        let cancellation = self.cancellation;
        let mut progress = |_: &tree_sitter::QueryCursorState| {
            if cancellation.is_cancelled() {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let options = tree_sitter::QueryCursorOptions::new().progress_callback(&mut progress);
        let mut matches = cursor.matches_with_options(
            query,
            parent_tree.root_node(),
            SnapshotTextProvider(self.snapshot),
            options,
        );
        while let Some(query_match) = matches.next() {
            if self.cancellation.is_cancelled() {
                return false;
            }
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
                        language_name = node_text(self.snapshot, capture.node.byte_range());
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
                if range.start >= range.end {
                    continue;
                }
                let key = InjectionKey::new(depth, &language, &range);
                // 保留层（`seen` 预置其键）与重复命中的变化区间：同一注入只收集一次。
                if !self.seen.insert(key) {
                    continue;
                }
                let old_tree = self.old_trees.remove(&key);
                let Some(tree) = parse_tree(
                    &language,
                    self.snapshot,
                    old_tree.as_ref(),
                    Some(range.clone()),
                    self.cancellation,
                ) else {
                    if self.cancellation.is_cancelled() {
                        return false;
                    }
                    continue;
                };
                // 嵌套注入只在其树变化的区间内递归（含编辑区间按层范围裁剪），其余复用保留层。
                if let Some(old_tree) = old_tree {
                    let mut sub_ranges: Vec<Range<usize>> = old_tree
                        .changed_ranges(&tree)
                        .map(|changed| changed.start_byte..changed.end_byte)
                        .collect();
                    if let Some(edits) = self.edits {
                        for edit in edits {
                            let clipped = edit.start.max(range.start)..edit.end.min(range.end);
                            if clipped.start < clipped.end {
                                sub_ranges.push(clipped);
                            }
                        }
                    }
                    for sub_range in merge_changed_ranges(sub_ranges) {
                        if !self.collect(&language, &tree, sub_range, depth + 1) {
                            return false;
                        }
                    }
                } else if !self.collect(&language, &tree, range.clone(), depth + 1) {
                    return false;
                }
                self.layers.push(SyntaxLayer {
                    language: language.clone(),
                    tree,
                    range,
                    depth,
                });
            }
        }
        true
    }
}

/// 合并字节变化区间（编辑区间 ∪ 树 `changed_ranges`，按起点有序、可相邻或重叠）为互不相交的列表。
fn merge_changed_ranges(ranges: impl IntoIterator<Item = Range<usize>>) -> Vec<Range<usize>> {
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

/// 提取一次文本变更在新坐标下的字节区间（`requires_reset` 时返回 None，调用方按全文处理）。
pub(crate) fn edit_ranges(changes: &TextChangeBatch) -> Option<Vec<Range<usize>>> {
    (!changes.requires_reset()).then(|| {
        changes
            .patch()
            .edits()
            .iter()
            .map(|edit| edit.new_range().start().get()..edit.new_range().end().get())
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use zcv_text::{ByteOffset, Edit, TextRange, TransactionMetadata};

    use super::*;
    use crate::test::{parsed_syntax, rust_buffer};

    /// 测试共用：按给定编辑把语法映射推进到新版本（插值 + 后台解析 + 安装）。
    fn edit_and_reparse(buffer: &mut zcv_text::Buffer, syntax: &mut SyntaxMap, edits: Vec<Edit>) {
        let subscription = buffer.subscribe();
        let old_snapshot = buffer.snapshot();
        buffer.edit(edits, TransactionMetadata::default()).unwrap();
        let new_snapshot = buffer.snapshot();
        let changes = subscription.consume();
        syntax.interpolate(&old_snapshot, &new_snapshot, &changes);
        let parsed = syntax
            .snapshot()
            .reparse(
                &new_snapshot,
                edit_ranges(&changes).as_deref(),
                &ParseCancellation::default(),
            )
            .expect("测试解析不应取消");
        assert!(syntax.did_parse(parsed));
    }

    /// 测试共用：按深度与语言名查找注入层（缺失时 panic）。
    fn find_layer<'a>(layers: &'a [SyntaxLayer], depth: u32, language: &str) -> &'a SyntaxLayer {
        layers
            .iter()
            .find(|layer| layer.depth == depth && layer.language.name() == language)
            .unwrap_or_else(|| panic!("缺少 depth {depth} 的注入层 {language}"))
    }

    #[test]
    fn go_annotated_string_creates_sql_injection_layer() {
        let source = "package main\nconst query = /* sql */ `SELECT name FROM users`\n";
        let (_, syntax) = parsed_syntax("main.go", source);
        let snapshot = syntax.snapshot();
        let sql = find_layer(snapshot.injection_layers(), 1, "SQL");

        assert_eq!(&source[sql.range.clone()], "SELECT name FROM users");
    }

    #[test]
    fn unchanged_injection_layers_survive_sibling_edits_without_reparse() {
        // 编辑块 1 内容：块 2/3 的注入层与编辑前逐位相同（原样保留，不重新查询也不重新解析）。
        let source = "\
```rust
let a = 1;
```
```python
print(1)
```
```javascript
const x = 2;
```
";
        let (mut buffer, mut syntax) = parsed_syntax("README.md", source);
        assert_eq!(
            syntax.snapshot().injection_layers().len(),
            3,
            "三个围栏块各产生一个注入层"
        );

        // 变长编辑（"1" → "42"）：等长替换对 tree-sitter 增量解析不可见，无法用于断言"重新解析"。
        let one = source.find('1').unwrap();
        let subscription = buffer.subscribe();
        let old_snapshot = buffer.snapshot();
        buffer
            .edit(
                [Edit::replace(
                    TextRange::new(ByteOffset::new(one), ByteOffset::new(one + 1)).unwrap(),
                    "42",
                )],
                TransactionMetadata::default(),
            )
            .unwrap();
        let new_snapshot = buffer.snapshot();
        let changes = subscription.consume();
        syntax.interpolate(&old_snapshot, &new_snapshot, &changes);
        // 插值后的层：保留层此刻就是插值树本身（未重新解析）。
        let interpolated: Vec<SyntaxLayer> = syntax.snapshot().injection_layers().to_vec();
        let parsed = syntax
            .snapshot()
            .reparse(
                &new_snapshot,
                edit_ranges(&changes).as_deref(),
                &ParseCancellation::default(),
            )
            .expect("测试解析不应取消");
        assert!(syntax.did_parse(parsed));

        let snapshot = syntax.snapshot();
        let layers = snapshot.injection_layers();
        assert_eq!(layers.len(), 3, "层数量应保持不变");
        // 未受影响的注入层：插值树与最终树结构完全相同（原样保留，未重新解析；
        // 坐标同为编辑后版本，changed_ranges 是有效的结构比较）。
        let python_interpolated = find_layer(&interpolated, 1, "Python");
        let python = find_layer(layers, 1, "Python");
        assert_eq!(
            python
                .tree
                .changed_ranges(&python_interpolated.tree)
                .count(),
            0,
            "Python 层不应被重新解析"
        );
        let javascript_interpolated = find_layer(&interpolated, 1, "JavaScript");
        let javascript = find_layer(layers, 1, "JavaScript");
        assert_eq!(
            javascript
                .tree
                .changed_ranges(&javascript_interpolated.tree)
                .count(),
            0,
            "JavaScript 层不应被重新解析"
        );
        // 受影响的注入层：重新收集后范围跟随编辑后的文本坐标（"1" → "42" 使内容区终点 +1）。
        // 注意：等长或同构内容编辑不改变树结构，`changed_ranges` 对此不可见，用范围坐标断言。
        let rust = find_layer(layers, 1, "Rust");
        assert_eq!(rust.range, 8..20, "Rust 层范围应映射到编辑后的内容区");
    }

    #[test]
    fn dbg3_ws() {
        let source = "```rust\nlet a = 1;\n```\n";
        let (mut buffer, mut syntax) = parsed_syntax("README.md", source);
        let fence = source.find("```rust").unwrap() + 3;
        let subscription = buffer.subscribe();
        let old_snapshot = buffer.snapshot();
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(fence + 4), " ").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();
        let new_snapshot = buffer.snapshot();
        let changes = subscription.consume();
        syntax.interpolate(&old_snapshot, &new_snapshot, &changes);
        for l in syntax.snapshot().injection_layers() {
            eprintln!(
                "interpolated: depth={} lang={} range={:?}",
                l.depth,
                l.language.name(),
                l.range
            );
        }
        let snap_before = syntax.snapshot();
        let old_main = snap_before.root_tree().unwrap().clone();
        let parsed = syntax
            .snapshot()
            .reparse(
                &new_snapshot,
                edit_ranges(&changes).as_deref(),
                &ParseCancellation::default(),
            )
            .expect("解析不应取消");
        eprintln!(
            "tree changed: {:?}",
            old_main
                .changed_ranges(parsed.root_tree().unwrap())
                .map(|r| r.start_byte..r.end_byte)
                .collect::<Vec<_>>()
        );
        // 新主树中 code_fence_content 节点的实际范围
        let root = parsed.root_tree().unwrap().root_node();
        eprintln!("sexp: {}", root.to_sexp());
        assert!(syntax.did_parse(parsed));
        for l in syntax.snapshot().injection_layers() {
            eprintln!(
                "final: depth={} lang={} range={:?}",
                l.depth,
                l.language.name(),
                l.range
            );
        }
    }

    #[test]
    fn dbg2_fence() {
        let source = "```rust\nlet a = 1;\n```\n```python\nprint(1)\n```\n";
        let (mut buffer, mut syntax) = parsed_syntax("README.md", source);
        let rust = source.find("rust").unwrap();
        let subscription = buffer.subscribe();
        let old_snapshot = buffer.snapshot();
        buffer
            .edit(
                [Edit::replace(
                    TextRange::new(ByteOffset::new(rust), ByteOffset::new(rust + 4)).unwrap(),
                    "python",
                )],
                TransactionMetadata::default(),
            )
            .unwrap();
        let new_snapshot = buffer.snapshot();
        let changes = subscription.consume();
        eprintln!(
            "changes edits: {:?}",
            changes
                .patch()
                .edits()
                .iter()
                .map(|e| (
                    e.old_range().start().get()..e.old_range().end().get(),
                    e.new_range().start().get()..e.new_range().end().get()
                ))
                .collect::<Vec<_>>()
        );
        syntax.interpolate(&old_snapshot, &new_snapshot, &changes);
        for l in syntax.snapshot().injection_layers() {
            eprintln!(
                "interpolated: depth={} lang={} range={:?}",
                l.depth,
                l.language.name(),
                l.range
            );
        }
        let snap_before = syntax.snapshot();
        let old_main = snap_before.root_tree().unwrap().clone();
        let parsed = syntax
            .snapshot()
            .reparse(
                &new_snapshot,
                edit_ranges(&changes).as_deref(),
                &ParseCancellation::default(),
            )
            .expect("解析不应取消");
        eprintln!(
            "tree changed: {:?}",
            old_main
                .changed_ranges(parsed.root_tree().unwrap())
                .map(|r| r.start_byte..r.end_byte)
                .collect::<Vec<_>>()
        );
        // 新主树的 fenced 区域实际文本
        let root = parsed.root_tree().unwrap().root_node();
        eprintln!(
            "main sexp head: {}",
            root.to_sexp().chars().take(200).collect::<String>()
        );
        assert!(syntax.did_parse(parsed));
        for l in syntax.snapshot().injection_layers() {
            eprintln!(
                "final: depth={} lang={} range={:?}",
                l.depth,
                l.language.name(),
                l.range
            );
        }
    }

    #[test]
    fn fence_language_edit_replaces_the_injection_layer_without_duplicates() {
        // 围栏语言 rust → python：内容范围未变但注入语言变了。
        // 重新收集的注入必须替换保留的旧层；同深同内容范围只允许一层。
        let source = "```rust\nlet a = 1;\n```\n```python\nprint(1)\n```\n";
        let (mut buffer, mut syntax) = parsed_syntax("README.md", source);
        let rust = source.find("rust").unwrap();
        edit_and_reparse(
            &mut buffer,
            &mut syntax,
            vec![Edit::replace(
                TextRange::new(ByteOffset::new(rust), ByteOffset::new(rust + 4)).unwrap(),
                "python",
            )],
        );

        let snapshot = syntax.snapshot();
        let layers = snapshot.injection_layers();
        assert_eq!(
            layers
                .iter()
                .filter(|l| l.language.name() == "Rust")
                .count(),
            0,
            "围栏语言改为 python 后不应残留 Rust 层"
        );
        assert_eq!(
            layers
                .iter()
                .filter(|l| l.language.name() == "Python")
                .count(),
            2,
            "块 1 改为 python 后应有块 1 与块 2 两个 Python 层"
        );
        // 块 1 的内容范围（编辑后坐标）只被一个层覆盖。
        let text = buffer.snapshot();
        let content = text
            .slice_byte_range(ByteOffset::ZERO, text.len_bytes())
            .unwrap();
        let content_start = content.as_str().find("let a = 1;").unwrap();
        let covering = layers
            .iter()
            .filter(|l| l.range.start <= content_start && content_start < l.range.end)
            .count();
        assert_eq!(covering, 1, "块 1 内容只应被一个注入层覆盖");
    }

    #[test]
    fn whitespace_fence_edit_keeps_the_layer_without_duplicates() {
        // "```rust" → "``` rust"：语言名 trim 后相同，保留层直接复用，不产生重复层。
        let source = "```rust\nlet a = 1;\n```\n";
        let (mut buffer, mut syntax) = parsed_syntax("README.md", source);
        let fence = source.find("```rust").unwrap() + 3;
        let subscription = buffer.subscribe();
        let old_snapshot = buffer.snapshot();
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(fence + 4), " ").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();
        let new_snapshot = buffer.snapshot();
        let changes = subscription.consume();
        syntax.interpolate(&old_snapshot, &new_snapshot, &changes);
        let interpolated = syntax.snapshot().injection_layers().to_vec();
        let parsed = syntax
            .snapshot()
            .reparse(
                &new_snapshot,
                edit_ranges(&changes).as_deref(),
                &ParseCancellation::default(),
            )
            .expect("测试解析不应取消");
        assert!(syntax.did_parse(parsed));

        let snapshot = syntax.snapshot();
        let layers = snapshot.injection_layers();
        let rust_layers: Vec<_> = layers
            .iter()
            .filter(|l| l.language.name() == "Rust")
            .collect();
        assert_eq!(rust_layers.len(), 1, "语言名未变不应产生重复层");
        // 保留层：插值树与最终树结构完全相同（未重新解析、未重复收集）。
        let interpolated_rust = find_layer(&interpolated, 1, "Rust");
        assert_eq!(
            rust_layers[0]
                .tree
                .changed_ranges(&interpolated_rust.tree)
                .count(),
            0,
            "内容未变时注入树应原样保留"
        );
    }

    #[test]
    fn added_and_removed_fenced_blocks_update_layers_incrementally() {
        let source = "```rust\nlet a = 1;\n```\n```python\nprint(1)\n```\n";
        let (mut buffer, mut syntax) = parsed_syntax("README.md", source);
        let end = buffer.len_bytes();

        // 追加新围栏块 → 新增注入层。
        edit_and_reparse(
            &mut buffer,
            &mut syntax,
            vec![Edit::insert(end, "```javascript\nconst x = 2;\n```\n").unwrap()],
        );
        let snapshot = syntax.snapshot();
        let layers = snapshot.injection_layers();
        assert_eq!(layers.len(), 3, "追加围栏块后应新增一层");
        assert!(
            layers.iter().any(|l| l.language.name() == "JavaScript"),
            "新增层应为 JavaScript"
        );

        // 删除中间的 python 围栏块 → 对应注入层消失，其余保留。
        let text = buffer.snapshot();
        let all = text
            .slice_byte_range(ByteOffset::ZERO, text.len_bytes())
            .unwrap();
        let python_start = all.as_str().find("```python").unwrap();
        let python_end = all.as_str().find("```\n```javascript").unwrap() + 3;
        edit_and_reparse(
            &mut buffer,
            &mut syntax,
            vec![Edit::replace(
                TextRange::new(ByteOffset::new(python_start), ByteOffset::new(python_end)).unwrap(),
                String::new(),
            )],
        );
        let snapshot = syntax.snapshot();
        let layers = snapshot.injection_layers();
        assert_eq!(layers.len(), 2, "删除围栏块后应回到两层");
        assert!(
            layers.iter().all(|l| l.language.name() != "Python"),
            "Python 注入层应随围栏块删除而消失"
        );
    }

    #[test]
    fn nested_injection_recollects_only_within_inner_changed_ranges() {
        // 围栏 markdown 块内的 inline 注入（depth 2）：
        // 编辑内层段落，嵌套层经递归按内层树的变化区间重收集，兄弟注入层不受影响。
        let source = "```markdown\nHello *world*\n```\n```rust\nlet a = 1;\n```\n";
        let (mut buffer, mut syntax) = parsed_syntax("README.md", source);
        let layers_before = syntax.snapshot().injection_layers().to_vec();
        assert_eq!(
            layers_before.len(),
            3,
            "外层 markdown + rust + 内层 inline 共三层"
        );
        find_layer(&layers_before, 2, "Markdown Inline");

        let world = source.find("world").unwrap();
        let subscription = buffer.subscribe();
        let old_snapshot = buffer.snapshot();
        buffer
            .edit(
                [Edit::replace(
                    TextRange::new(ByteOffset::new(world), ByteOffset::new(world + 5)).unwrap(),
                    "planets",
                )],
                TransactionMetadata::default(),
            )
            .unwrap();
        let new_snapshot = buffer.snapshot();
        let changes = subscription.consume();
        syntax.interpolate(&old_snapshot, &new_snapshot, &changes);
        let interpolated = syntax.snapshot().injection_layers().to_vec();
        let parsed = syntax
            .snapshot()
            .reparse(
                &new_snapshot,
                edit_ranges(&changes).as_deref(),
                &ParseCancellation::default(),
            )
            .expect("测试解析不应取消");
        assert!(syntax.did_parse(parsed));

        let snapshot = syntax.snapshot();
        let layers = snapshot.injection_layers();
        assert_eq!(layers.len(), 3, "层数量应保持不变");
        // 内层 inline 注入重新收集后范围覆盖编辑后的新文本（"world" → "planets" +2 字节）。
        // 内容编辑不改变树结构，`changed_ranges` 对此不可见，用范围坐标断言。
        let inline = find_layer(layers, 2, "Markdown Inline");
        let text = buffer.snapshot();
        let planets = text
            .slice_byte_range(ByteOffset::ZERO, text.len_bytes())
            .unwrap()
            .as_str()
            .find("planets")
            .expect("编辑后的文本应包含 planets");
        assert!(
            inline.range.start <= planets && planets < inline.range.end,
            "内层 inline 注入范围应覆盖编辑后的新文本"
        );
        let rust = find_layer(layers, 1, "Rust");
        let rust_interpolated = find_layer(&interpolated, 1, "Rust");
        assert_eq!(
            rust.tree.changed_ranges(&rust_interpolated.tree).count(),
            0,
            "兄弟注入层应原样保留"
        );
    }

    #[test]
    fn layers_for_range_queries_across_depths_and_points() {
        // 按 (深度, 起点) 有序 + 同深不交：范围查询覆盖跨深度命中、
        // 空查询（点包含）、以及起点恰在查询起点/终点的边界。
        let source = "\
```markdown
Hello *world*
```
```rust
let a = 1;
```
```rust
let b = 2;
```
";
        let (buffer, syntax) = parsed_syntax("README.md", source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let all = snapshot
            .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
            .unwrap();
        let full = 0..snapshot.len_bytes().get();

        // 全文查询：外层 markdown 层 + 内层 inline + 两个 Rust 层全部命中。
        let names: Vec<&str> = syntax
            .layers_for_range(&full)
            .map(|layer| layer.language.name())
            .collect();
        assert!(names.contains(&"Markdown"));
        assert!(names.contains(&"Markdown Inline"));
        assert_eq!(names.iter().filter(|name| **name == "Rust").count(), 2);

        // 空查询（光标点）：命中的层必须包含该点。
        let world = all.as_str().find("*world*").unwrap() + 1;
        let point_hits: Vec<_> = syntax
            .layers_for_range(&(world..world))
            .map(|layer| layer.language.name())
            .collect();
        assert!(point_hits.contains(&"Markdown"), "外层层应包含光标点");
        assert!(
            point_hits.contains(&"Markdown Inline"),
            "内层 inline 应包含光标点"
        );

        // 只查询第二个 Rust 块：第一个 Rust 层不得命中。
        let second_rust = all.as_str().rfind("let b = 2;").unwrap();
        let first_rust = all.as_str().find("let a = 1;").unwrap();
        let rust_hits: Vec<_> = syntax
            .layers_for_range(&(first_rust..second_rust + 3))
            .map(|layer| layer.language.name())
            .collect();
        assert_eq!(
            rust_hits.iter().filter(|name| **name == "Rust").count(),
            2,
            "区间覆盖两个 Rust 块时应都命中"
        );
        let single_rust: Vec<_> = syntax
            .layers_for_range(&(second_rust..second_rust + 1))
            .map(|layer| layer.language.name())
            .collect();
        assert_eq!(
            single_rust.iter().filter(|name| **name == "Rust").count(),
            1,
            "只落在第二个块内的区间不应命中第一个 Rust 层"
        );
    }

    #[test]
    fn syntax_ancestor_uses_the_smallest_layer() {
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
    }

    #[test]
    fn syntax_snapshots_share_immutable_state_until_interpolation() {
        let (mut buffer, mut syntax) = rust_buffer("fn main() {}\n");
        let first = syntax.snapshot();
        let second = syntax.snapshot();
        assert!(Arc::ptr_eq(&first.state, &second.state));

        let subscription = buffer.subscribe();
        let old_snapshot = buffer.snapshot();
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();
        let new_snapshot = buffer.snapshot();
        syntax.interpolate(&old_snapshot, &new_snapshot, &subscription.consume());

        let interpolated = syntax.snapshot();
        assert!(!Arc::ptr_eq(&first.state, &interpolated.state));
        assert_eq!(first.version(), old_snapshot.version());
        assert_eq!(interpolated.version(), new_snapshot.version());
    }

    #[test]
    fn time_sliced_parse_resumes_and_matches_single_pass_result() {
        // 极短预算（1ns）强制多次中断-恢复：分片完成的树必须与一次性解析逐位等价。
        // 小文件可能在首个 progress callback 前就完成，用 ~500 行代码确保多次分片。
        let mut source = String::new();
        for index in 0..500 {
            source.push_str(&format!(
                "fn function_{index}(value: i32) -> i32 {{\n    let result = value * {index};\n    result\n}}\n\n"
            ));
        }
        let buffer =
            zcv_text::Buffer::from_text(source.to_owned(), zcv_text::BufferConfig::default())
                .unwrap();
        let snapshot = buffer.snapshot();
        let language = crate::registry::language_for_file(Path::new("main.rs"), None)
            .expect("Rust 语言应可加载");
        let cancellation = ParseCancellation::default();

        // 一次性解析（对照）。
        let single =
            parse_tree(&language, &snapshot, None, None, &cancellation).expect("一次性解析应成功");

        // 分片解析：逐片推进直到完成。
        let mut parser = IncrementalParser::new();
        let mut slices = 0usize;
        let sliced = loop {
            slices += 1;
            assert!(slices < 10_000, "分片解析应在有限片数内完成（预算过小？）");
            match parser.parse_slice(
                &language,
                &snapshot,
                None,
                None,
                &cancellation,
                Duration::from_nanos(1),
            ) {
                Some(tree) => break tree,
                None => {
                    assert!(!cancellation.is_cancelled());
                    continue;
                }
            }
        };
        assert!(slices > 1, "1ns 预算应产生多次分片，实际 {slices} 片");
        assert_eq!(
            single.changed_ranges(&sliced).count(),
            0,
            "分片恢复解析应与一次性解析结构等价"
        );
    }

    #[test]
    fn time_sliced_parse_aborts_on_cancellation() {
        let source = "fn main() { let value = 1; }\n";
        let buffer =
            zcv_text::Buffer::from_text(source.to_owned(), zcv_text::BufferConfig::default())
                .unwrap();
        let snapshot = buffer.snapshot();
        let language = crate::registry::language_for_file(Path::new("main.rs"), None)
            .expect("Rust 语言应可加载");
        let cancellation = ParseCancellation::default();
        let mut parser = IncrementalParser::new();
        // 首片后取消：后续片必须放弃，不产生死循环。
        let _ = parser.parse_slice(
            &language,
            &snapshot,
            None,
            None,
            &cancellation,
            Duration::from_nanos(1),
        );
        cancellation.cancel();
        assert!(
            parser
                .parse_slice(
                    &language,
                    &snapshot,
                    None,
                    None,
                    &cancellation,
                    Duration::from_nanos(1),
                )
                .is_none()
        );
    }

    #[test]
    fn cancelled_parse_produces_no_installable_snapshot() {
        let buffer =
            zcv_text::Buffer::from_text("fn main() {}\n".to_owned(), Default::default()).unwrap();
        let snapshot = buffer.snapshot();
        let mut syntax = SyntaxMap::new(&snapshot);
        let first_line = "fn main() {}";
        syntax.set_language_for_file(Path::new("main.rs"), Some(first_line), &snapshot);
        let cancellation = ParseCancellation::default();
        cancellation.cancel();

        assert!(
            syntax
                .snapshot()
                .reparse(&snapshot, None, &cancellation)
                .is_none()
        );
    }

    #[test]
    fn unchanged_injection_reuses_its_tree_across_parent_edits() {
        let source = "<style>.item { color: red; }</style><script>let value = 1;</script>";
        let (mut buffer, mut syntax) = parsed_syntax("index.html", source);
        let subscription = buffer.subscribe();
        let old_snapshot = buffer.snapshot();
        let red = source.find("red").unwrap();
        buffer
            .edit(
                [Edit::replace(
                    TextRange::new(ByteOffset::new(red), ByteOffset::new(red + 3)).unwrap(),
                    "blue",
                )],
                TransactionMetadata::default(),
            )
            .unwrap();
        let new_snapshot = buffer.snapshot();
        let changes = subscription.consume();
        syntax.interpolate(&old_snapshot, &new_snapshot, &changes);
        let interpolated_tree = syntax
            .snapshot()
            .injection_layers()
            .iter()
            .find(|layer| layer.language.name() == "JavaScript")
            .expect("HTML 应包含 JavaScript 注入层")
            .tree
            .clone();
        let parsed = syntax
            .snapshot()
            .reparse(
                &new_snapshot,
                edit_ranges(&changes).as_deref(),
                &ParseCancellation::default(),
            )
            .expect("测试解析不应取消");
        assert!(syntax.did_parse(parsed));

        let parsed_tree = syntax
            .snapshot()
            .injection_layers()
            .iter()
            .find(|layer| layer.language.name() == "JavaScript")
            .expect("编辑 CSS 后 JavaScript 注入层应保留")
            .tree
            .clone();
        assert_eq!(interpolated_tree.changed_ranges(&parsed_tree).count(), 0);
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
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(start), "\"文本\"").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();
        let new_snapshot = buffer.snapshot();
        let changes = subscription.consume();
        syntax.interpolate(&old_snapshot, &new_snapshot, &changes);
        let parsed = syntax
            .snapshot()
            .reparse(
                &new_snapshot,
                edit_ranges(&changes).as_deref(),
                &ParseCancellation::default(),
            )
            .expect("测试解析不应取消");
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
        buffer
            .edit(
                [
                    Edit::replace(
                        TextRange::new(ByteOffset::new(first), ByteOffset::new(first + 1)).unwrap(),
                        "\"一\"",
                    ),
                    Edit::replace(
                        TextRange::new(ByteOffset::new(second), ByteOffset::new(second + 1))
                            .unwrap(),
                        "\"二\"",
                    ),
                ],
                TransactionMetadata::default(),
            )
            .unwrap();
        let new_snapshot = buffer.snapshot();
        let changes = subscription.consume();
        syntax.interpolate(&old_snapshot, &new_snapshot, &changes);
        let parsed = syntax
            .snapshot()
            .reparse(
                &new_snapshot,
                edit_ranges(&changes).as_deref(),
                &ParseCancellation::default(),
            )
            .expect("测试解析不应取消");
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
    fn stale_parse_result_cannot_replace_interpolated_tree() {
        let (mut buffer, mut syntax) = rust_buffer("fn main() {}\n");
        let stale_parse = syntax.snapshot();
        let subscription = buffer.subscribe();
        let old_snapshot = buffer.snapshot();
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();
        let new_snapshot = buffer.snapshot();
        syntax.interpolate(&old_snapshot, &new_snapshot, &subscription.consume());

        let stale = stale_parse
            .reparse(&old_snapshot, None, &ParseCancellation::default())
            .expect("测试解析不应取消");
        assert!(!syntax.did_parse(stale));
        assert_eq!(syntax.snapshot().version(), new_snapshot.version());
    }
}
