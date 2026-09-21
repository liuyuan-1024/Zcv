use std::collections::{HashMap, HashSet};
use std::ops::{ControlFlow, Range};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use tree_sitter::StreamingIterator;
use zcv_text::{Anchor, BufferVersion, ByteOffset, Snapshot, TextChangeBatch, TextRange};

use crate::Language;
use crate::registry::LanguageRegistry;
use crate::structure::FoldRange;
use crate::tree_sitter_utils::{
    IncrementalParser, PARSE_TIME_SLICE, ParseCancellation, QueryCursorHandle,
    SnapshotTextProvider, drop_offloaded, edit_tree, node_text, parse_tree, ranges_overlap,
};

/// 可增量更新的语法状态。
///
/// `parsed_version` 表示 Tree 真正完成解析的版本；
/// `interpolated_version` 表示旧树已经通过 `InputEdit` 推进到的文本版本。
/// 两者分离后，前台可以立即使用坐标正确的旧树，真正的增量解析则交给后台完成。
pub(crate) struct SyntaxMap {
    registry: Arc<LanguageRegistry>,
    language: Option<Arc<Language>>,
    state: Arc<SyntaxState>,
    parsed_version: BufferVersion,
    interpolated_version: BufferVersion,
    /// 最近一次插值对应的文本快照：推进语法树坐标与折叠范围时作为旧坐标基准。
    interpolated_snapshot: Snapshot,
}

/// 整源折叠候选缓存条目：绑定语法版本，随语法状态克隆重置。
#[derive(Debug)]
struct FoldRangeCacheEntry {
    version: BufferVersion,
    ranges: Arc<[FoldRange]>,
}

#[derive(Debug, Default)]
struct SyntaxState {
    tree: Option<tree_sitter::Tree>,
    injections: Vec<SyntaxLayer>,
    /// 最近一次解析安装的 capture 全局表（见 `SyntaxSnapshot::rebuild_capture_table`）。
    capture_names: Arc<[Arc<str>]>,
    capture_index_by_language: HashMap<&'static str, Arc<[u32]>>,
    /// 整源折叠候选缓存；渲染路径按范围过滤，不再逐行重跑查询。
    fold_ranges: Mutex<Option<FoldRangeCacheEntry>>,
}

impl Clone for SyntaxState {
    fn clone(&self) -> Self {
        Self {
            tree: self.tree.clone(),
            injections: self.injections.clone(),
            capture_names: Arc::clone(&self.capture_names),
            capture_index_by_language: self.capture_index_by_language.clone(),
            // 派生缓存不随状态克隆复制：克隆意味着语法状态将要变化，旧候选立即失效。
            fold_ranges: Mutex::new(None),
        }
    }
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
    /// 语法树真正完成解析的版本；增量解析按它推导编辑区间。
    parsed_version: BufferVersion,
    /// 树坐标已经推进到的文本版本；查询一律以它为有效版本。
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

/// 语法层内容：已解析的注入树，或当前注册表无法解析的待处理注入。
#[derive(Clone, Debug)]
pub(crate) enum SyntaxLayerContent {
    Parsed {
        language: Arc<Language>,
        tree: tree_sitter::Tree,
    },
    /// 注入查询声明了语言名，但注册表中没有对应语言。
    ///
    /// 保留待处理层而不是静默丢弃，使层列表忠实反映注入点；
    /// 范围仍按锚点跟随编辑。
    Pending { language_name: Arc<str> },
}

#[derive(Clone, Debug)]
pub(crate) struct SyntaxLayer {
    pub(crate) depth: u32,
    /// 注入内容在文本中的范围；用锚点保存，跨编辑不手工映射。
    pub(crate) range: Range<Anchor>,
    pub(crate) content: SyntaxLayerContent,
}

impl SyntaxLayer {
    pub(crate) fn language(&self) -> Option<&Arc<Language>> {
        match &self.content {
            SyntaxLayerContent::Parsed { language, .. } => Some(language),
            SyntaxLayerContent::Pending { .. } => None,
        }
    }

    pub(crate) fn language_name(&self) -> &str {
        match &self.content {
            SyntaxLayerContent::Parsed { language, .. } => language.name(),
            SyntaxLayerContent::Pending { language_name } => language_name,
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct InjectionKey {
    depth: u32,
    language: Arc<str>,
    start: usize,
    end: usize,
}

impl InjectionKey {
    fn new(depth: u32, language: &str, range: &Range<usize>) -> Self {
        Self {
            depth,
            language: Arc::from(language),
            start: range.start,
            end: range.end,
        }
    }
}

/// 把当前快照中的字节范围转成吸收边界插入的锚点范围。
fn anchor_range(snapshot: &Snapshot, range: &Range<usize>) -> Option<Range<Anchor>> {
    let text_range =
        TextRange::new(ByteOffset::new(range.start), ByteOffset::new(range.end)).ok()?;
    Some(Anchor::range_outside(snapshot.version(), text_range))
}

/// 把层的锚点范围解析到当前快照的字节范围。
fn layer_bytes(snapshot: &Snapshot, layer: &SyntaxLayer) -> Option<Range<usize>> {
    let start = layer.range.start.resolve_in(snapshot).ok()?;
    let end = layer.range.end.resolve_in(snapshot).ok()?;
    Some(start.get()..end.get())
}

/// 取同一 Buffer 生命周期内的坐标编辑批次。
///
/// 坐标索引不衰减，插值/解析版本始终落在其覆盖范围内；
/// `None` 说明调用方把别的 Buffer 的快照传了进来，属于不变量破坏，必须显式失败而不是丢弃全部层做全文重跑。
fn coordinate_edits_or_fail(snapshot: &Snapshot, since: BufferVersion) -> TextChangeBatch {
    snapshot.coordinate_edits_since(since).unwrap_or_else(|| {
        panic!(
            "语法快照版本 {since:?} 不在当前 Buffer 的坐标索引覆盖范围内；插值与解析只能在同一个 Buffer 生命周期内推进"
        )
    })
}

impl SyntaxMap {
    pub(crate) fn language(&self) -> Option<&Language> {
        self.language.as_deref()
    }

    /// 当前语言的共享句柄；调用方需要在 `SyntaxMap` 之外持有语言时使用。
    pub(crate) fn language_arc(&self) -> Option<Arc<Language>> {
        self.language.clone()
    }

    pub(crate) fn registry(&self) -> Arc<LanguageRegistry> {
        Arc::clone(&self.registry)
    }

    pub(crate) fn new(registry: Arc<LanguageRegistry>, snapshot: &Snapshot) -> Self {
        Self {
            registry,
            language: None,
            state: empty_syntax_state(),
            parsed_version: snapshot.version(),
            interpolated_version: snapshot.version(),
            interpolated_snapshot: snapshot.clone(),
        }
    }

    pub(crate) fn set_language_for_file(
        &mut self,
        path: &Path,
        first_line: Option<&str>,
        snapshot: &Snapshot,
    ) -> bool {
        self.set_language(self.registry.language_for_file(path, first_line), snapshot)
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
        self.interpolated_snapshot = snapshot.clone();
        true
    }

    /// 只把旧树推进到新坐标，不在调用线程执行解析。
    ///
    /// 编辑区间由 `interpolated_version` 与当前快照推导；调用方无需携带订阅批次，
    /// 因此快照读取与 observer 唤醒可以各自幂等推进（对齐 Zed `Buffer::snapshot()`）。
    pub(crate) fn interpolate(&mut self, new_snapshot: &Snapshot) {
        // 同版本重复调用必须保持原树；否则空批次会被当成整体重置。
        if new_snapshot.version() == self.interpolated_version {
            return;
        }
        if self.language.is_none() {
            self.parsed_version = new_snapshot.version();
            self.interpolated_version = new_snapshot.version();
            self.interpolated_snapshot = new_snapshot.clone();
            return;
        }

        // 增量编辑走不衰减坐标索引：带文本 EditLog 被裁剪后仍可用。
        // 插值版本始终属于当前 Buffer 生命周期，坐标索引必然覆盖；
        // 缺失即不变量失败，不得回退为丢弃全部语法状态并全文重跑。
        let changes = coordinate_edits_or_fail(new_snapshot, self.interpolated_version);

        let old_snapshot = &self.interpolated_snapshot;
        let state = Arc::make_mut(&mut self.state);
        let mut tree = state.tree.take();
        if tree
            .as_mut()
            .is_some_and(|tree| !edit_tree(tree, old_snapshot, new_snapshot, &changes))
            && let Some(old_tree) = tree.take()
        {
            drop_offloaded(old_tree);
        }
        let mut invalid_layers = Vec::new();
        let mut retained = Vec::with_capacity(state.injections.len());
        for mut layer in std::mem::take(&mut state.injections) {
            // 锚点自行跟随编辑；范围被删空说明注入点已消失，必须丢弃该层。
            let non_empty =
                layer_bytes(new_snapshot, &layer).is_some_and(|bytes| bytes.start < bytes.end);
            // 已解析层把增量编辑应用到树上；待处理层没有树。
            let tree_ok = match &mut layer.content {
                SyntaxLayerContent::Parsed { tree, .. } => {
                    edit_tree(tree, old_snapshot, new_snapshot, &changes)
                }
                SyntaxLayerContent::Pending { .. } => true,
            };
            if non_empty && tree_ok {
                retained.push(layer);
            } else {
                invalid_layers.push(layer);
            }
        }
        state.injections = retained;
        if !invalid_layers.is_empty() {
            drop_offloaded(invalid_layers);
        }

        state.tree = tree;
        self.interpolated_version = new_snapshot.version();
        self.interpolated_snapshot = new_snapshot.clone();
    }

    pub(crate) fn snapshot(&self) -> SyntaxSnapshot {
        SyntaxSnapshot {
            language: self.language.clone(),
            state: Arc::clone(&self.state),
            parsed_version: self.parsed_version,
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
        self.parsed_version = parsed.parsed_version;
        true
    }
}

impl SyntaxSnapshot {
    /// 空语法快照（无语言、无树）：语言匹配前或未安装语法时的占位，查询一律返回空。
    pub fn empty(version: BufferVersion) -> Self {
        Self {
            language: None,
            state: empty_syntax_state(),
            parsed_version: version,
            version,
        }
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    pub(crate) fn can_query(&self, range: &Range<usize>, text: &Snapshot) -> bool {
        text.version() == self.version
            && range.start <= range.end
            && range.end <= text.len_bytes().get()
    }

    /// 返回整源折叠候选，按语法版本缓存。
    ///
    /// 候选只依赖当前树与文本版本；
    /// 同一版本的渲染查询共享同一份结果，消费方按范围过滤即可，避免为每个可见行重跑 Tree-sitter 查询。
    pub(crate) fn cached_fold_ranges(&self, text: &Snapshot) -> Arc<[FoldRange]> {
        let mut cache = self
            .state
            .fold_ranges
            .lock()
            .expect("折叠候选缓存锁不得中毒");
        if let Some(entry) = cache.as_ref()
            && entry.version == self.version
        {
            return Arc::clone(&entry.ranges);
        }
        let ranges: Arc<[FoldRange]> = self.query_fold_ranges(text).into();
        *cache = Some(FoldRangeCacheEntry {
            version: self.version,
            ranges: Arc::clone(&ranges),
        });
        ranges
    }

    /// 返回与范围相交的语法层（主语言层 + 已解析注入层）。
    ///
    /// 注入层范围用锚点保存，查询时按 `text` 解析成本次查询坐标；
    /// 未解析的待处理层没有树，不参与查询。
    pub(crate) fn layers_for_range<'a>(
        &'a self,
        text: &'a Snapshot,
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
                .filter_map(|layer| resolved_layer_ref(text, layer, range)),
        )
    }

    /// 同步执行真正的 tree-sitter 增量解析。
    /// 调用方必须把该方法放到后台，再通过 `SyntaxMap::did_parse` 安装结果。
    ///
    /// `edits` 是本次编辑在新坐标下的字节区间：tree-sitter 的 `changed_ranges` 对等长替换（parser 直接复用旧叶子）不可见，必须用文本编辑区间兜底。
    /// 变化区间 = 编辑区间 ∪ 树变化区间，两者都不覆盖的区域注入层原样保留。
    pub(crate) fn reparse(
        mut self,
        snapshot: &Snapshot,
        registry: &Arc<LanguageRegistry>,
        cancellation: &ParseCancellation,
    ) -> Option<Self> {
        if cancellation.is_cancelled() {
            return None;
        }
        // 编辑区间按上一次真正完成解析的版本推导，优先走不衰减坐标索引：
        // 带文本 EditLog 被预算裁剪后仍能给出精确增量，不静默退化为全文。
        let changes = coordinate_edits_or_fail(snapshot, self.parsed_version);
        let edit_list = edit_ranges(&changes);
        let edits = Some(edit_list.as_slice());
        let Some(language) = self.language.as_ref() else {
            self.state = empty_syntax_state();
            self.version = snapshot.version();
            self.parsed_version = snapshot.version();
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
                // 首次解析或语言切换：无旧树可比，全文收集（显式边界，不来自增量失败）。
                _ => std::iter::once(0..snapshot.len_bytes().get()).collect(),
            };
            state.tree = new_tree;

            let old_injections = std::mem::take(&mut state.injections);
            // 范围与任何变化区间相交的已解析旧层进入复用表（供增量解析）；其余原样保留。
            // 先把锚点范围解析成当前快照字节；无法解析的层直接丢弃。
            let mut seen = HashSet::new();
            let mut old_trees = HashMap::new();
            for layer in old_injections {
                let Some(byte_range) = layer_bytes(snapshot, &layer) else {
                    drop_offloaded(layer);
                    continue;
                };
                if byte_range.start >= byte_range.end {
                    drop_offloaded(layer);
                    continue;
                }
                let key = InjectionKey::new(layer.depth, layer.language_name(), &byte_range);
                if changed
                    .iter()
                    .any(|range| ranges_overlap(&byte_range, range))
                {
                    if let SyntaxLayerContent::Parsed { tree, .. } = layer.content {
                        old_trees.insert(key, tree);
                    }
                } else {
                    seen.insert(key);
                    state.injections.push(layer);
                }
            }

            let mut collected = Vec::new();
            if let Some(tree) = state.tree.as_ref() {
                let mut collector = InjectionCollector {
                    snapshot,
                    registry,
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
                let Some(old_bytes) = layer_bytes(snapshot, &layer) else {
                    continue;
                };
                let replaced = collected.iter().any(|new| {
                    new.depth == layer.depth
                        && layer_bytes(snapshot, new)
                            .is_some_and(|new_bytes| ranges_overlap(&new_bytes, &old_bytes))
                });
                if !replaced {
                    final_layers.push(layer);
                }
            }
            final_layers.extend(collected);
            // 按 (深度, 解析后的字节区间) 稳定排序；锚点本身不可比较。
            final_layers.sort_unstable_by_key(|layer| {
                (
                    layer.depth,
                    layer_bytes(snapshot, layer).map(|bytes| (bytes.start, bytes.end)),
                )
            });
            state.injections = final_layers;
        }
        self.version = snapshot.version();
        self.parsed_version = snapshot.version();
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
            if let Some(language) = layer.language() {
                add_language(language);
            }
        }
        let state = Arc::make_mut(&mut self.state);
        state.capture_names = Arc::from(names);
        state.capture_index_by_language = index_by_language;
    }

    /// 语言局部 capture index -> 快照全局 index 的映射（高亮收集用）。
    pub(crate) fn capture_index_table(&self, language: &Language) -> Option<&Arc<[u32]>> {
        self.state.capture_index_by_language.get(language.name())
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

/// 把一条注入层解析到当前快照；待处理层或与查询范围不相交时返回 None。
fn resolved_layer_ref<'a>(
    text: &'a Snapshot,
    layer: &'a SyntaxLayer,
    range: &Range<usize>,
) -> Option<SyntaxLayerRef<'a>> {
    let SyntaxLayerContent::Parsed { language, tree } = &layer.content else {
        return None;
    };
    let bytes = layer_bytes(text, layer)?;
    // 空查询按“点包含”语义：起点恰在查询点的层也算命中。
    let intersects = if range.is_empty() {
        bytes.start <= range.start && range.start < bytes.end
    } else {
        ranges_overlap(&bytes, range)
    };
    intersects.then_some(SyntaxLayerRef {
        language: language.as_ref(),
        tree,
        depth: layer.depth,
    })
}

/// 按变化区间收集注入：查询限定在 `range` 内，旧树按注入键复用做增量解析，未变化的嵌套注入通过 `seen`（含全部保留层键）跳过，不重复收集。
struct InjectionCollector<'a> {
    snapshot: &'a Snapshot,
    registry: &'a Arc<LanguageRegistry>,
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
            let language = self.registry.language_for_injection(&language_name);
            for range in content_ranges {
                if range.start >= range.end {
                    continue;
                }
                let Some(anchors) = anchor_range(self.snapshot, &range) else {
                    continue;
                };
                let Some(language) = language.as_ref() else {
                    // 未注册的注入语言：保留待处理层，不再静默丢弃。
                    let key = InjectionKey::new(depth, &language_name, &range);
                    if !self.seen.insert(key) {
                        continue;
                    }
                    self.layers.push(SyntaxLayer {
                        depth,
                        range: anchors,
                        content: SyntaxLayerContent::Pending {
                            language_name: Arc::from(language_name.as_str()),
                        },
                    });
                    continue;
                };
                let key = InjectionKey::new(depth, language.name(), &range);
                // 保留层（`seen` 预置其键）与重复命中的变化区间：同一注入只收集一次。
                if !self.seen.insert(key.clone()) {
                    continue;
                }
                let old_tree = self.old_trees.remove(&key);
                let Some(tree) = parse_tree(
                    language,
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
                        if !self.collect(language.as_ref(), &tree, sub_range, depth + 1) {
                            return false;
                        }
                    }
                } else if !self.collect(language.as_ref(), &tree, range.clone(), depth + 1) {
                    return false;
                }
                self.layers.push(SyntaxLayer {
                    depth,
                    range: anchors,
                    content: SyntaxLayerContent::Parsed {
                        language: language.clone(),
                        tree,
                    },
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

/// 提取一次文本变更在新坐标下的字节区间。
pub(crate) fn edit_ranges(changes: &TextChangeBatch) -> Vec<Range<usize>> {
    changes
        .patch()
        .edits()
        .iter()
        .map(|edit| edit.new_range().start().get()..edit.new_range().end().get())
        .collect()
}

#[cfg(test)]
#[path = "test/syntax_map_tests.rs"]
mod tests;
