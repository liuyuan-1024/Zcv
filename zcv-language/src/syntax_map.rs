use std::collections::{HashMap, HashSet};
use std::ops::{ControlFlow, Range};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use tree_sitter::StreamingIterator;
use zcv_text::{BufferVersion, Snapshot, TextChangeBatch};

use crate::Language;
use crate::registry::{language_for_file, language_for_injection};
use crate::tree_sitter_utils::{
    ParseCancellation, QueryCursorHandle, SnapshotTextProvider, drop_offloaded, edit_tree,
    encloses, map_range_through_changes, node_text, parse_tree, range_touches,
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

    pub(crate) fn can_query(&self, range: &Range<usize>, text: &Snapshot) -> bool {
        text.version() == self.version
            && range.start <= range.end
            && range.end <= text.len_bytes().get()
    }

    /// 返回与范围相交的语法层（主语言层 + 注入层），零堆分配。
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
        main.into_iter().chain(
            self.state
                .injections
                .iter()
                .filter(move |layer| range_touches(&layer.range, range))
                .map(|layer| SyntaxLayerRef {
                    language: layer.language.as_ref(),
                    tree: &layer.tree,
                    depth: layer.depth,
                }),
        )
    }

    /// 同步执行真正的 tree-sitter 增量解析。
    /// 调用方必须把该方法放到后台，再通过 `SyntaxMap::did_parse` 安装结果。
    pub(crate) fn reparse(
        mut self,
        snapshot: &Snapshot,
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
            state.tree = parse_tree(language, snapshot, state.tree.as_ref(), None, cancellation);
            if cancellation.is_cancelled() {
                return None;
            }
            let old_injections = std::mem::take(&mut state.injections);
            let mut old_trees = old_injections
                .into_iter()
                .map(|layer| {
                    (
                        InjectionKey::new(layer.depth, &layer.language, &layer.range),
                        layer.tree,
                    )
                })
                .collect::<HashMap<_, _>>();
            let mut seen = HashSet::new();
            if let Some(tree) = state.tree.as_ref() {
                let mut collector = InjectionCollector {
                    snapshot,
                    old_trees: &mut old_trees,
                    seen: &mut seen,
                    layers: &mut state.injections,
                    cancellation,
                };
                if !collector.collect(language, tree, 1) {
                    return None;
                }
            }
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
    /// 使 `HighlightSpan::capture` 在快照内跨语言唯一；同时构建每语言的
    /// 局部 index -> 全局 index 映射，高亮收集时直接数组索引、零哈希查找。
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

struct InjectionCollector<'a> {
    snapshot: &'a Snapshot,
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
                if !self.collect(&language, &tree, depth + 1) {
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

/// 测试共用：按 Rust 文件解析文本，返回 buffer 与已安装解析结果的语法映射。
#[cfg(test)]
pub(crate) fn rust_buffer(text: &str) -> (zcv_text::Buffer, SyntaxMap) {
    parsed_syntax("main.rs", text)
}

/// 测试共用：按给定路径解析文本，返回 buffer 与已安装解析结果的语法映射。
#[cfg(test)]
pub(crate) fn parsed_syntax(path: &str, text: &str) -> (zcv_text::Buffer, SyntaxMap) {
    let buffer =
        zcv_text::Buffer::from_text(text.to_owned(), zcv_text::BufferConfig::default()).unwrap();
    let snapshot = buffer.snapshot();
    let mut syntax = SyntaxMap::new(&snapshot);
    let first_line = snapshot
        .slice_line(zcv_text::Line::ZERO)
        .unwrap()
        .as_str()
        .trim_end_matches(['\r', '\n'])
        .to_owned();
    syntax.set_language_for_file(Path::new(path), Some(&first_line), &snapshot);
    let parsed = syntax
        .snapshot()
        .reparse(&snapshot, &ParseCancellation::default())
        .expect("测试解析不应取消");
    assert!(syntax.did_parse(parsed));
    (buffer, syntax)
}

#[cfg(test)]
mod tests {
    use zcv_text::{ByteOffset, Edit, TextRange, TransactionMetadata};

    use super::*;

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
                .reparse(&snapshot, &cancellation)
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
        syntax.interpolate(&old_snapshot, &new_snapshot, &subscription.consume());
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
            .reparse(&new_snapshot, &ParseCancellation::default())
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
        syntax.interpolate(&old_snapshot, &new_snapshot, &subscription.consume());
        let parsed = syntax
            .snapshot()
            .reparse(&new_snapshot, &ParseCancellation::default())
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
        syntax.interpolate(&old_snapshot, &new_snapshot, &subscription.consume());
        let parsed = syntax
            .snapshot()
            .reparse(&new_snapshot, &ParseCancellation::default())
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
            .reparse(&old_snapshot, &ParseCancellation::default())
            .expect("测试解析不应取消");
        assert!(!syntax.did_parse(stale));
        assert_eq!(syntax.snapshot().version(), new_snapshot.version());
    }
}
