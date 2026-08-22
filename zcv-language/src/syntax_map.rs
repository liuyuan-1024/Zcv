use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use tree_sitter::StreamingIterator;
use zcv_engine::{BufferVersion, Snapshot, TextChangeBatch};

use crate::Language;
use crate::registry::{language_for_file, language_for_injection};
use crate::tree_sitter_utils::{
    QueryCursorHandle, SnapshotTextProvider, drop_offloaded, edit_tree, encloses,
    map_range_through_changes, node_text, parse_tree, range_touches,
};

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
    /// 最近一次解析安装的 capture 全局表（见 `SyntaxSnapshot::rebuild_capture_table`）。
    capture_names: Arc<[Arc<str>]>,
    capture_index_by_language: Arc<HashMap<&'static str, Arc<[u32]>>>,
}

/// 与一个 Buffer 版本绑定的不可变语法快照。
///
/// 字段对 crate 内可见：高亮与结构查询模块以 `impl SyntaxSnapshot` 扩展查询方法。
#[derive(Clone, Debug)]
pub struct SyntaxSnapshot {
    pub(crate) language: Option<Language>,
    pub(crate) tree: Option<tree_sitter::Tree>,
    pub(crate) injections: Vec<SyntaxLayer>,
    pub(crate) version: BufferVersion,
    /// 主语言与全部注入语言的 capture 名字全局表；index 跨语言唯一。
    capture_names: Arc<[Arc<str>]>,
    /// 语言名 -> 该语言局部 capture index 到全局 index 的映射（高亮收集时数组索引，零分配）。
    pub(crate) capture_index_by_language: Arc<HashMap<&'static str, Arc<[u32]>>>,
}

impl Drop for SyntaxSnapshot {
    fn drop(&mut self) {
        // 树与注入层是主要内存占用；最后一个引用消失时移交给后台线程释放，否则深树 dealloc 会卡住主线程（对齐 Zed）。
        if self.tree.is_some() || !self.injections.is_empty() {
            drop_offloaded((self.tree.take(), std::mem::take(&mut self.injections)));
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SyntaxLayer {
    pub(crate) language: Language,
    pub(crate) tree: tree_sitter::Tree,
    pub(crate) range: Range<usize>,
    pub(crate) depth: u32,
}

impl SyntaxMap {
    pub(crate) fn language(&self) -> Option<&Language> {
        self.language.as_ref()
    }

    pub fn new(snapshot: &Snapshot) -> Self {
        Self {
            language: None,
            tree: None,
            injections: Vec::new(),
            parsed_version: snapshot.version(),
            interpolated_version: snapshot.version(),
            capture_names: Arc::from([]),
            capture_index_by_language: Arc::new(HashMap::new()),
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
        // 语言切换时旧树/旧注入层同样移交给后台线程释放。
        if let Some(old_tree) = self.tree.take() {
            drop_offloaded(old_tree);
        }
        let old_layers = std::mem::take(&mut self.injections);
        if !old_layers.is_empty() {
            drop_offloaded(old_layers);
        }
        self.capture_names = Arc::from([]);
        self.capture_index_by_language = Arc::new(HashMap::new());
        self.parsed_version = snapshot.version();
        self.interpolated_version = snapshot.version();
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
                // 坐标换算失败的树无法继续插值，移交给后台线程释放。
                if let Some(old_tree) = tree.take() {
                    drop_offloaded(old_tree);
                }
            }
            self.injections.retain_mut(|layer| {
                if !edit_tree(&mut layer.tree, old_snapshot, new_snapshot, changes) {
                    return false;
                }
                layer.range = map_range_through_changes(layer.range.clone(), changes);
                layer.range.start < layer.range.end
            });
        } else {
            // 编辑不满足增量条件时整树重置：旧树与旧注入层移交给后台线程释放。
            if let Some(old_tree) = tree.take() {
                drop_offloaded(old_tree);
            }
            let old_layers = std::mem::take(&mut self.injections);
            if !old_layers.is_empty() {
                drop_offloaded(old_layers);
            }
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
            capture_names: Arc::clone(&self.capture_names),
            capture_index_by_language: Arc::clone(&self.capture_index_by_language),
        }
    }

    pub fn did_parse(&mut self, mut parsed: SyntaxSnapshot) -> bool {
        if parsed.version != self.interpolated_version
            || parsed.language.as_ref().map(Language::name)
                != self.language.as_ref().map(Language::name)
        {
            return false;
        }
        // 被替换的旧树/旧注入层移交给后台线程释放，避免每次解析完成时主线程卡顿。
        if let Some(old_tree) = std::mem::replace(&mut self.tree, parsed.tree.take()) {
            drop_offloaded(old_tree);
        }
        let old_layers =
            std::mem::replace(&mut self.injections, std::mem::take(&mut parsed.injections));
        if !old_layers.is_empty() {
            drop_offloaded(old_layers);
        }
        self.capture_names = std::mem::take(&mut parsed.capture_names);
        self.capture_index_by_language = std::mem::take(&mut parsed.capture_index_by_language);
        self.parsed_version = parsed.version;
        true
    }
}

impl SyntaxSnapshot {
    /// 空语法快照（无语言、无树）：语言匹配前或未安装语法时的占位，查询一律返回空。
    pub fn empty(version: BufferVersion) -> Self {
        Self {
            language: None,
            tree: None,
            injections: Vec::new(),
            version,
            capture_names: Arc::from([]),
            capture_index_by_language: Arc::new(HashMap::new()),
        }
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    pub fn has_language(&self) -> bool {
        self.language.is_some()
    }

    pub fn language_at(&self, offset: usize, text: &Snapshot) -> Option<&'static str> {
        let range = offset..offset;
        self.can_query(&range, text).then_some(())?;
        self.layers_for_range(&range)
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
        let main = match (&self.language, &self.tree) {
            (Some(language), Some(tree)) => Some(SyntaxLayerRef {
                language,
                tree,
                range: tree.root_node().byte_range(),
                depth: 0,
            }),
            _ => None,
        };
        main.into_iter().chain(
            self.injections
                .iter()
                .filter(move |layer| range_touches(&layer.range, range))
                .map(|layer| SyntaxLayerRef {
                    language: &layer.language,
                    tree: &layer.tree,
                    range: layer.range.clone(),
                    depth: layer.depth,
                }),
        )
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
        for layer in &self.injections {
            add_language(&layer.language);
        }
        self.capture_names = Arc::from(names);
        self.capture_index_by_language = Arc::new(index_by_language);
    }

    /// 语言局部 capture index -> 快照全局 index 的映射（高亮收集用）。
    pub(crate) fn capture_index_table(&self, language: &Language) -> Option<&Arc<[u32]>> {
        self.capture_index_by_language.get(language.name())
    }
}

pub(crate) struct SyntaxLayerRef<'a> {
    pub(crate) language: &'a Language,
    pub(crate) tree: &'a tree_sitter::Tree,
    pub(crate) range: Range<usize>,
    pub(crate) depth: u32,
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

/// 测试共用：按 Rust 文件解析文本，返回 buffer 与已安装解析结果的语法映射。
#[cfg(test)]
pub(crate) fn rust_buffer(text: &str) -> (zcv_engine::Buffer, SyntaxMap) {
    parsed_syntax("main.rs", text)
}

/// 测试共用：按给定路径解析文本，返回 buffer 与已安装解析结果的语法映射。
#[cfg(test)]
pub(crate) fn parsed_syntax(path: &str, text: &str) -> (zcv_engine::Buffer, SyntaxMap) {
    let buffer =
        zcv_engine::Buffer::from_text(text.to_owned(), zcv_engine::BufferConfig::default())
            .unwrap();
    let snapshot = buffer.snapshot();
    let mut syntax = SyntaxMap::new(&snapshot);
    syntax.set_language_for_file(Path::new(path), &snapshot);
    let parsed = syntax.snapshot().reparse(&snapshot);
    assert!(syntax.did_parse(parsed));
    (buffer, syntax)
}

#[cfg(test)]
mod tests {
    use zcv_engine::{ByteOffset, Edit, TextRange, TransactionMetadata};

    use super::*;

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
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(start), "\"文本\"").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();
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

        let stale = stale_parse.reparse(&old_snapshot);
        assert!(!syntax.did_parse(stale));
        assert_eq!(syntax.snapshot().version(), new_snapshot.version());
    }
}
