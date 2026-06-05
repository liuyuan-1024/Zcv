//! tree-sitter-md provider：block + inline 两套 grammar 串起来。
//!
//! tree-sitter-md 把 Markdown 拆成 `LANGUAGE`（block：标题 / 列表 / 代码块 / 引用块）与 `INLINE_LANGUAGE`（行内 emphasis / strong / code_span / link 等）
//! 两套 grammar。block tree 里的 `(inline)` 节点是 inline grammar 的入口——
//! 取它的字节切片重 parse，把 inline highlights 叠到 block 之上。
//!
//! ## 为什么不复用 [`super::common::HighlightWorker`]
//!
//! 那个 worker 是「一棵 tree 一份 query」的统一形态——所有 Tier 1 单文法语言共用。
//! markdown 需要两棵 tree（block + 每个 inline 节点一棵 inline）+ 两份 query 合并。
//! 强行塞进通用 worker 会让其它语言每次都背 `Option<InlinePass>`的字段与代码路径分支；不划算。
//! 这里独立写一个 [`MarkdownWorker`]，复用 common 模块的 [`SharedConfig`] / [`collect_spans`] / [`reset_cursor_range`] 三个细粒度构件，整体复用度仍够。
//!
//! ## 当前形态：块树增量 + inline 全量重 parse
//!
//! - **块树增量**：worker 缓存上一轮的 block `Tree` + `Snapshot`，`on_edit` 走 `translate_edits` + 逐条 `Tree::edit` + `Parser::parse(..., Some(&old))`， 失败回退到 `run_full` 全量重解析。
//! 等价性护栏：`tests::incremental_matches_full_after_edits`。
//! - **`apply_pending_edit` 折叠**：批量编辑事件除最后一条都只推块树指针（`Tree::edit`），最后一条走完整 `on_edit`——N 次按键合并成 1 次 reparse + 1 次 sink push，与 [`super::common::HighlightWorker`] 同形。
//! - **inline 仍全量**：每次产物期都把块树里所有 `inline` 节点逐个用 inline parser 重 parse。每段切片本身就小（一行 / 一段），代价远小于块树；按 inline 节点级别再做增量收益微薄、复杂度高，留作后续 viewport-aware 优化的一部分。
//! - **viewport hint 仍未接**：`set_viewport` 默认 no-op。
//! markdown 文档 < 100 KB 时全文产物可接受；接 viewport 留给后一步。
//!
//! ## 手册 §十四 例外
//!
//! 手册原本把 "injection / combined parsers" 整体列为非目标。
//! markdown 的 block+inline 是**同一语言族的两套配套 grammar**，不是真正的跨语言嵌入（代码栅栏里跑 rust / HTML 里跑 JS 仍归非目标）。已在手册侧明确这条例外。

use std::sync::{Arc, OnceLock};

use tree_sitter::{Node, Parser, QueryCursor, QueryError, Tree};
use zom_engine::{BufferVersion, ByteOffset, ChangeSet, Snapshot, TextRange};

use crate::syntax::LanguageId;
use crate::syntax::payload::HighlightSpan;
use crate::syntax::provider::{BufferHandle, HighlightProvider};
use crate::syntax::providers::common::{
    SharedConfig, build_shared_config, build_shared_config_with_normalize, collect_spans,
    reset_cursor_range, translate_edits,
};
use crate::syntax::sink::HighlightSink;

fn markdown_block_config() -> Result<Arc<SharedConfig>, &'static QueryError> {
    static CELL: OnceLock<Result<Arc<SharedConfig>, QueryError>> = OnceLock::new();
    CELL.get_or_init(|| {
        // block grammar 沿用 common 的默认归一化：text.title → markup.heading、
        // text.literal → markup.raw.block、text.uri / text.reference 同步落到
        // markup.link.*——这些恰好就是 block 端期望的语义。
        build_shared_config(
            tree_sitter_md::LANGUAGE.into(),
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
        )
        .map(Arc::new)
    })
    .as_ref()
    .map(|c| c.clone())
}

fn markdown_inline_config() -> Result<Arc<SharedConfig>, &'static QueryError> {
    static CELL: OnceLock<Result<Arc<SharedConfig>, QueryError>> = OnceLock::new();
    CELL.get_or_init(|| {
        build_shared_config_with_normalize(
            tree_sitter_md::INLINE_LANGUAGE.into(),
            tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
            normalize_md_inline,
        )
        .map(Arc::new)
    })
    .as_ref()
    .map(|c| c.clone())
}

/// inline grammar 专用归一化：与 block 不同，`text.literal` 指 `code_span`，
/// `text.emphasis` / `text.strong` 是行内强调。映射到主题 (`onedark.toml`) 已
/// 配好的 `markup.*` 词汇表。
fn normalize_md_inline(name: &str) -> &str {
    match name {
        "text.literal" => "markup.raw.inline",
        "text.emphasis" => "markup.italic",
        "text.strong" => "markup.bold",
        "text.uri" => "markup.link.url",
        "text.reference" => "markup.link.text",
        other => other,
    }
}

pub fn new_provider() -> MarkdownWorker {
    let block = markdown_block_config().expect("tree-sitter-md block 高亮配置必须构建");
    let inline = markdown_inline_config().expect("tree-sitter-md inline 高亮配置必须构建");
    MarkdownWorker::new(LanguageId::new("markdown"), block, inline)
}

/// markdown provider 的 worker。
///
/// 内部并行持两份 [`SharedConfig`]（block + inline），两组 [`Parser`] /
/// [`QueryCursor`]，以及块树缓存（`block_tree` + `last_snapshot`），用于支撑
/// `Tree::edit` 增量链。inline 不缓存——每次产物期就近重 parse。
pub struct MarkdownWorker {
    language_id: LanguageId,
    block_config: Arc<SharedConfig>,
    inline_config: Arc<SharedConfig>,
    block_parser: Parser,
    inline_parser: Parser,
    block_cursor: QueryCursor,
    inline_cursor: QueryCursor,
    sink_slot: Option<HighlightSink>,
    /// 上一次解析出的块树。`None` 表示尚未首次解析或上一轮已回退；下一次
    /// `on_edit` 走全量重解析把这两个槽都填回去。
    block_tree: Option<Tree>,
    /// 与 `block_tree` 对应的 Snapshot：增量路径计算 InputEdit **旧端** Point
    /// 时需要它（新端 Point 用 on_edit 收到的新 snapshot）。
    last_snapshot: Option<Snapshot>,
    /// 当前 desktop 上报的 viewport hint。`Some(range)` 时：
    /// - block 查询用 `block_cursor.set_byte_range` 限定到 viewport；
    /// - inline pass 跳过不与 viewport 相交的 `inline` 节点；
    /// - 产物按 `sink.replace_range(version, range, spans)` 投递；
    /// - `set_viewport` 改值时立即 `reissue_viewport_query`——不重 parse、
    ///   仅重 query，让滚动 1–2 帧内补齐。
    ///
    /// `None` 时回退到全文 `replace_all` 路径。
    viewport_hint: Option<TextRange>,
}

impl std::fmt::Debug for MarkdownWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MarkdownWorker")
            .field("language", &self.language_id)
            .finish_non_exhaustive()
    }
}

impl MarkdownWorker {
    fn new(
        language_id: LanguageId,
        block_config: Arc<SharedConfig>,
        inline_config: Arc<SharedConfig>,
    ) -> Self {
        let mut block_parser = Parser::new();
        block_parser
            .set_language(&block_config.language)
            .expect("set_language block 失败：tree-sitter-md ABI 不匹配");
        let mut inline_parser = Parser::new();
        inline_parser
            .set_language(&inline_config.language)
            .expect("set_language inline 失败：tree-sitter-md ABI 不匹配");
        Self {
            language_id,
            block_config,
            inline_config,
            block_parser,
            inline_parser,
            block_cursor: QueryCursor::new(),
            inline_cursor: QueryCursor::new(),
            sink_slot: None,
            block_tree: None,
            last_snapshot: None,
            viewport_hint: None,
        }
    }

    /// 全量解析当前 snapshot，按 viewport hint 决定 query 与 sink 投递路径。
    ///
    /// **viewport-aware**：parse 阶段必须走全文（tree-sitter 要构建整棵树），但
    /// query 阶段按 [`Self::viewport_hint`] 分支：
    ///
    /// - `Some(range)`：先推一份**空 `ReplaceAll`** 在 sink 上把 layer 锚到本
    ///   版本（清掉上一份高亮），再就 viewport 跑 block + inline 联合 query 并
    ///   以 `ReplaceRange` 投递视口段 spans。视口外保持空，等滚动 / 编辑再补齐。
    /// - `None`：全文 query + `ReplaceAll`。
    fn run_full(&mut self, buffer: &BufferHandle, sink: &HighlightSink) {
        let snapshot = buffer.snapshot();
        let version = snapshot.version();
        let text = match snapshot.slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes()) {
            Ok(t) => t.into_text().into_owned(),
            Err(_) => {
                self.block_tree = None;
                self.last_snapshot = None;
                sink.replace_all(version, Vec::new());
                return;
            }
        };
        let bytes = text.as_bytes();

        let Some(block_tree) = self.block_parser.parse(bytes, None) else {
            self.block_tree = None;
            self.last_snapshot = None;
            sink.replace_all(version, Vec::new());
            return;
        };

        match self.viewport_hint {
            Some(range) => {
                sink.replace_all(version, Vec::new());
                let merged = self.produce_spans(&block_tree, bytes, Some(range));
                sink.replace_range(version, range, merged);
            }
            None => {
                let merged = self.produce_spans(&block_tree, bytes, None);
                sink.replace_all(version, merged);
            }
        }
        self.block_tree = Some(block_tree);
        self.last_snapshot = Some(snapshot);
    }

    /// 尝试增量重解析块树并推送 spans；返回 `false` 表示需要调用方走全量解析。
    ///
    /// 失败路径与 [`super::common::HighlightWorker::try_incremental`] 同形：
    /// 缓存缺失 / 版本不连续 / InputEdit 翻译失败 / `parse` 返回 `None`。
    /// 任一失败都把 `block_tree` / `last_snapshot` 槽清空让调用方走 `run_full` 兜底；
    /// 不在这里直接推 spans，避免半增量半全量的 sink 状态。
    fn try_incremental(
        &mut self,
        buffer: &BufferHandle,
        change: &ChangeSet,
        sink: &HighlightSink,
        new_version: BufferVersion,
    ) -> bool {
        let (Some(old_snapshot), Some(mut tree)) =
            (self.last_snapshot.take(), self.block_tree.take())
        else {
            return false;
        };
        // 增量路径要求缓存版本与本次事件版本恰好相邻——中间若漏看事件，tree.edit
        // 链断裂，结果不可信。
        if old_snapshot.version().get().saturating_add(1) != new_version.get() {
            return false;
        }
        let new_snapshot = buffer.snapshot();
        let version = new_snapshot.version();

        let input_edits = match translate_edits(change, &old_snapshot, &new_snapshot) {
            Some(edits) => edits,
            None => return false,
        };
        for ie in &input_edits {
            tree.edit(ie);
        }

        let text = match new_snapshot.slice_byte_range(ByteOffset::ZERO, new_snapshot.len_bytes()) {
            Ok(t) => t.into_text().into_owned(),
            Err(_) => return false,
        };
        let bytes = text.as_bytes();

        let Some(new_tree) = self.block_parser.parse(bytes, Some(&tree)) else {
            return false;
        };

        match self.viewport_hint {
            Some(range) => {
                let merged = self.produce_spans(&new_tree, bytes, Some(range));
                sink.replace_range(version, range, merged);
            }
            None => {
                let merged = self.produce_spans(&new_tree, bytes, None);
                sink.replace_all(version, merged);
            }
        }
        self.block_tree = Some(new_tree);
        self.last_snapshot = Some(new_snapshot);
        true
    }

    /// 立刻就 `range` 跑一次 viewport-scoped block+inline query，
    /// 把结果作为 `ReplaceRange` 推给 sink——不重 parse、不刷 `block_tree` / `last_snapshot`。
    ///
    /// 仅在 worker 已有缓存（tree + snapshot + sink）时有效。
    /// `set_viewport` 在 hint 实际改变后触发，让滚动后新区域 1–2 帧内见高亮，而不必等下一次按键。
    fn reissue_viewport_query(&mut self, range: TextRange) {
        let (Some(tree), Some(snapshot), Some(sink)) = (
            self.block_tree.take(),
            self.last_snapshot.take(),
            self.sink_slot.clone(),
        ) else {
            return;
        };
        let version = snapshot.version();
        let text = match snapshot.slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes()) {
            Ok(t) => t.into_text().into_owned(),
            Err(_) => {
                // 还回去——读不到 bytes 不代表缓存失效。
                self.block_tree = Some(tree);
                self.last_snapshot = Some(snapshot);
                return;
            }
        };
        let bytes = text.as_bytes();
        let merged = self.produce_spans(&tree, bytes, Some(range));
        sink.replace_range(version, range, merged);
        self.block_tree = Some(tree);
        self.last_snapshot = Some(snapshot);
    }

    /// 给定块树与全文 bytes，产出 block + inline 合并后的最终 spans。
    ///
    /// `viewport`：
    /// - `Some(range)`：block query 用 `set_byte_range` 限定到 viewport；inline pass 跳过不与 viewport 相交的 `inline` 节点（节点内仍整段重 parse，保证 emphasis / strong 等成对结构不被截开），最后把 merged spans 按 viewport 裁剪一次——丢掉完全在外的，端点跨界的 clamp 到边界。
    /// - `None`：等价于全文路径。
    fn produce_spans(
        &mut self,
        block_tree: &Tree,
        bytes: &[u8],
        viewport: Option<TextRange>,
    ) -> Vec<(TextRange, HighlightSpan)> {
        if let Some(range) = viewport {
            self.block_cursor
                .set_byte_range(range.start().get()..range.end().get());
        } else {
            reset_cursor_range(&mut self.block_cursor);
        }
        let block_spans = collect_spans(
            &self.block_config,
            &mut self.block_cursor,
            block_tree,
            bytes,
        );
        reset_cursor_range(&mut self.block_cursor);

        let mut inline_spans: Vec<(TextRange, HighlightSpan)> = Vec::new();
        collect_inline_spans(
            &mut self.inline_parser,
            &mut self.inline_cursor,
            &self.inline_config,
            block_tree,
            bytes,
            viewport,
            &mut inline_spans,
        );

        let merged = merge_block_with_inline(block_spans, inline_spans);
        match viewport {
            Some(range) => clip_spans_to_range(merged, range),
            None => merged,
        }
    }
}

/// 把 spans 按 viewport range 裁剪：完全在外的丢掉，跨界的端点 clamp 到边界。
///
/// merge 之后调用一次。block_cursor 的 `set_byte_range` 已让 block spans 端点大概率落在 viewport 内；inline pass 整段重 parse 保证语法成对，
/// 但其 spans 端点可能略超 viewport。
/// 这里把超出部分裁回去，让 `sink.replace_range` 的「替换 `range` 内所有 spans」语义不被越界写污染。
fn clip_spans_to_range(
    spans: Vec<(TextRange, HighlightSpan)>,
    range: TextRange,
) -> Vec<(TextRange, HighlightSpan)> {
    let lo = range.start().get();
    let hi = range.end().get();
    spans
        .into_iter()
        .filter_map(|(r, s)| {
            let start = r.start().get().max(lo);
            let end = r.end().get().min(hi);
            if start >= end {
                return None;
            }
            TextRange::new(ByteOffset::new(start), ByteOffset::new(end))
                .ok()
                .map(|r| (r, s))
        })
        .collect()
}

impl HighlightProvider for MarkdownWorker {
    fn language(&self) -> LanguageId {
        self.language_id
    }

    fn attach(&mut self, buffer: BufferHandle, sink: HighlightSink) {
        self.sink_slot = Some(sink.clone());
        self.run_full(&buffer, &sink);
    }

    fn on_edit(&mut self, buffer: BufferHandle, change: &ChangeSet, version: BufferVersion) {
        let Some(sink) = self.sink_slot.clone() else {
            return;
        };
        // 中间批量事件的快路径：与 common::HighlightWorker 对齐——buffer 已是
        // 终态时跳过，下一条终态事件再触发实际产出。
        if buffer.version() != version {
            return;
        }
        if !self.try_incremental(&buffer, change, &sink, version) {
            self.run_full(&buffer, &sink);
        }
    }

    /// 中间编辑快路径：只走 `translate_edits + Tree::edit`，不重 parse、不 query、
    /// 不推 sink——与 [`super::common::HighlightWorker::apply_pending_edit`] 同形。
    ///
    /// 折叠路径：调度层把同一 buffer 连续编辑事件除最后一条都喂到这里，最后一条走 [`Self::on_edit`] 走完整 reparse + sink push。
    /// N 次按键合并成**一次** reparse 与 sink push。
    fn apply_pending_edit(
        &mut self,
        buffer: BufferHandle,
        change: &ChangeSet,
        version: BufferVersion,
    ) {
        // 与 on_edit 同样的中间事件守门：
        // buffer 已是终态时，没有"上一轮 snapshot" 可对——直接清空缓存让下一次 on_edit 走 run_full。
        if buffer.version() != version {
            self.block_tree = None;
            self.last_snapshot = None;
            return;
        }
        let (Some(old_snapshot), Some(mut tree)) =
            (self.last_snapshot.take(), self.block_tree.take())
        else {
            return;
        };
        if old_snapshot.version().get().saturating_add(1) != version.get() {
            return;
        }
        let new_snapshot = buffer.snapshot();
        let input_edits = match translate_edits(change, &old_snapshot, &new_snapshot) {
            Some(edits) => edits,
            None => return,
        };
        for ie in &input_edits {
            tree.edit(ie);
        }
        self.block_tree = Some(tree);
        self.last_snapshot = Some(new_snapshot);
    }

    fn detach(&mut self) {
        self.sink_slot = None;
    }

    fn set_viewport(&mut self, byte_range: Option<TextRange>) {
        if self.viewport_hint == byte_range {
            return;
        }
        self.viewport_hint = byte_range;
        // hint 改变后立即按新区域跑一次 query：滚动到新区域时，不必等下一次按键才看到高亮。
        // `None` 不触发——回退到全文模式由下一次 on_edit 重建。
        if let Some(range) = byte_range {
            self.reissue_viewport_query(range);
        }
    }
}

/// 走 block tree，对每一个 `inline` 节点取它的字节切片用 inline parser 重 parse，
/// 把 inline query 的结果按偏移平移回全局坐标后追加到 `out`。
///
/// 实现要点：
///
/// - **不递归进 inline 内部**：block grammar 不会在 `inline` 之下还套别的 `inline`，跳过即可。
/// - **inline parser 复用**：tree-sitter `Parser` 跨 parse 调用无残留状态，循环里多次 parse 不同切片是安全的。
/// - **cursor 范围重置**：每次 query 前 `reset_cursor_range`，否则上一个切片的 `set_byte_range` 会污染下一个切片的查询。
fn collect_inline_spans(
    inline_parser: &mut Parser,
    inline_cursor: &mut QueryCursor,
    inline_config: &SharedConfig,
    block_tree: &Tree,
    source: &[u8],
    viewport: Option<TextRange>,
    out: &mut Vec<(TextRange, HighlightSpan)>,
) {
    let mut cursor = block_tree.walk();
    'outer: loop {
        let node = cursor.node();
        if node.kind() == "inline" {
            if inline_node_intersects_viewport(&node, viewport) {
                parse_one_inline(
                    inline_parser,
                    inline_cursor,
                    inline_config,
                    node,
                    source,
                    out,
                );
            }
            // 不向 inline 内部下钻——它的子结构归 inline grammar 决定。
        } else if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                continue 'outer;
            }
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

/// inline 节点是否与 viewport 相交。无 viewport 时恒真。
///
/// 选 "整段相交" 而不是 "整段被包含" 是有意的：
/// emphasis / strong / link 等结构必须整段重 parse 才能让 inline grammar 看到成对的 `*` / `**` / `[]`。
/// 跨界 inline 节点最终 spans 端点会略超 viewport，靠 [`clip_spans_to_range`] 在 merge 后裁回。
fn inline_node_intersects_viewport(node: &Node<'_>, viewport: Option<TextRange>) -> bool {
    let Some(range) = viewport else { return true };
    let lo = range.start().get();
    let hi = range.end().get();
    node.start_byte() < hi && node.end_byte() > lo
}

fn parse_one_inline(
    inline_parser: &mut Parser,
    inline_cursor: &mut QueryCursor,
    inline_config: &SharedConfig,
    node: Node<'_>,
    source: &[u8],
    out: &mut Vec<(TextRange, HighlightSpan)>,
) {
    let start = node.start_byte();
    let end = node.end_byte();
    if end <= start || end > source.len() {
        return;
    }
    let slice = &source[start..end];
    let Some(tree) = inline_parser.parse(slice, None) else {
        return;
    };
    reset_cursor_range(inline_cursor);
    let local_spans = collect_spans(inline_config, inline_cursor, &tree, slice);
    for (range, span) in local_spans {
        let new_start = range.start().get() + start;
        let new_end = range.end().get() + start;
        if let Ok(global) = TextRange::new(ByteOffset::new(new_start), ByteOffset::new(new_end)) {
            out.push((global, span));
        }
    }
}

/// 把 inline spans「凿洞」叠到 block spans 之上。
///
/// 两边各自非重叠且按 start 排序；输出仍非重叠且按 start 排序。冲突区间内
/// **inline 胜出**——例如 `# Hello **world**` 里 "world" 段在 block 端是
/// `markup.heading`，inline 端是 `markup.bold`，最终落 `markup.bold`。
/// 这条优先级的选择见手册 §三/§十一：inner-wins 与 onedark 给 bold 配了独立
/// 字重 modifier，heading 内的强调仍应被读出。
///
/// 算法：扫一遍 block，对每个 block span，逐个用 cursor 切出与 inline span
/// 不重叠的 prefix 块，inline 段原样输出，剩余 suffix 留到下一轮 / block 末尾
/// 补齐。inline 跨多个 block 的情况实际不会出现（inline 总落在单个 block 的
/// `inline` 节点内），保守 clamp 一下即可。
fn merge_block_with_inline(
    block: Vec<(TextRange, HighlightSpan)>,
    inline: Vec<(TextRange, HighlightSpan)>,
) -> Vec<(TextRange, HighlightSpan)> {
    if inline.is_empty() {
        return block;
    }
    let mut result: Vec<(TextRange, HighlightSpan)> =
        Vec::with_capacity(block.len() + inline.len());
    let mut ii: usize = 0;

    for (block_range, block_span) in block {
        let b_start = block_range.start().get();
        let b_end = block_range.end().get();

        // 跳过 / 输出本 block 之前的 inline spans——它们落在所有 block 之外。
        while ii < inline.len() && inline[ii].0.end().get() <= b_start {
            result.push(inline[ii]);
            ii += 1;
        }

        let mut cursor = b_start;
        while ii < inline.len() && inline[ii].0.start().get() < b_end {
            let (irange, ispan) = inline[ii];
            let i_start = irange.start().get().max(cursor);
            let i_end = irange.end().get().min(b_end);

            if i_start > cursor
                && let Ok(r) = TextRange::new(ByteOffset::new(cursor), ByteOffset::new(i_start))
            {
                result.push((r, block_span));
            }
            if i_end > i_start
                && let Ok(r) = TextRange::new(ByteOffset::new(i_start), ByteOffset::new(i_end))
            {
                result.push((r, ispan));
            }
            cursor = i_end;

            if irange.end().get() > b_end {
                // inline 越过本 block 的右端——理论上不会发生（inline 都在
                // 单个 `inline` 节点内）。保守不前进 ii，让外层下一轮继续处理。
                break;
            }
            ii += 1;
        }

        if cursor < b_end
            && let Ok(r) = TextRange::new(ByteOffset::new(cursor), ByteOffset::new(b_end))
        {
            result.push((r, block_span));
        }
    }

    // 末尾 block 之后的 inline spans——挂在最后一个 block 之外的 inline 段。
    while ii < inline.len() {
        result.push(inline[ii]);
        ii += 1;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::BufferSyntaxState;
    use crate::syntax::SinkMessage;
    use crate::syntax::payload::syntax_layer_kind;
    use crate::syntax::providers::common::assert_lookup_matches_capture_names;
    use crate::syntax::{HighlightProvider, HighlightSpan, TokenModifiers};
    use zom_engine::{Buffer, BufferConfig, MetadataLayers};

    const SAMPLE: &str = "# heading\n\n- item\n\n```rust\nfn x() {}\n```\n";

    #[test]
    fn block_lookup_matches_query_capture_names() {
        let cfg = markdown_block_config().expect("markdown block 配置必须构建");
        assert_lookup_matches_capture_names(&cfg);
    }

    #[test]
    fn markdown_heading_uses_canonical_markup_name() {
        let buffer = Buffer::from_text("# heading\n".to_string(), BufferConfig::default()).unwrap();
        let mut layers = MetadataLayers::<HighlightSpan>::new();
        let provider: Box<dyn HighlightProvider> = Box::new(new_provider());

        let worker = std::sync::Arc::new(crate::syntax::SyntaxWorkerHandle::spawn());
        let state = BufferSyntaxState::attach(
            crate::BufferId::from_raw(1),
            LanguageId::new("markdown"),
            provider,
            &buffer,
            &mut layers,
            worker.clone(),
            None,
        );
        worker.wait_for_idle();
        state.drain_into_layers(buffer.version(), &mut layers);
        let names = layers
            .layer(&syntax_layer_kind())
            .expect("syntax layer 必须存在")
            .as_slice()
            .iter()
            .map(|range| range.metadata().name.as_str())
            .collect::<Vec<_>>();

        assert!(
            names.contains(&"markup.heading"),
            "markdown 标题应归一化为 markup.heading，实际为 {names:?}"
        );
    }

    /// 全语言路径烟测：attach SAMPLE 后 layer 非空。
    #[test]
    fn provider_emits_spans_for_sample() {
        let buffer = Buffer::from_text(SAMPLE.to_string(), BufferConfig::default()).unwrap();
        let mut layers = MetadataLayers::<HighlightSpan>::new();
        let provider: Box<dyn HighlightProvider> = Box::new(new_provider());
        let worker = std::sync::Arc::new(crate::syntax::SyntaxWorkerHandle::spawn());
        let state = BufferSyntaxState::attach(
            crate::BufferId::from_raw(1),
            LanguageId::new("markdown"),
            provider,
            &buffer,
            &mut layers,
            worker.clone(),
            None,
        );
        worker.wait_for_idle();
        state.drain_into_layers(buffer.version(), &mut layers);
        let layer = layers
            .layer(&syntax_layer_kind())
            .expect("syntax layer 必须存在");
        assert!(layer.len() > 0, "markdown provider 应至少产出一个 span");
    }

    // ============== inline grammar 接入护栏 ==============

    fn collect_names(text: &str) -> Vec<(usize, usize, &'static str)> {
        let buffer = Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap();
        let sink = HighlightSink::new();
        let mut worker = new_provider();
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
        let mut spans: Vec<(TextRange, HighlightSpan)> = Vec::new();
        for msg in sink.drain() {
            if let SinkMessage::ReplaceAll { spans: latest, .. } = msg {
                spans = latest;
            }
        }
        spans
            .into_iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str()))
            .collect()
    }

    fn names(text: &str) -> Vec<&'static str> {
        collect_names(text).into_iter().map(|(_, _, n)| n).collect()
    }

    fn span_for(text: &str, name: &str) -> Option<(usize, usize)> {
        collect_names(text)
            .into_iter()
            .find(|(_, _, n)| *n == name)
            .map(|(s, e, _)| (s, e))
    }

    #[test]
    fn inline_emphasis_emits_markup_italic() {
        let text = "para with *italic* word\n";
        let n = names(text);
        assert!(
            n.contains(&"markup.italic"),
            "*italic* 应产出 markup.italic，实际 {n:?}"
        );
    }

    #[test]
    fn inline_strong_emits_markup_bold() {
        let text = "para with **bold** word\n";
        let n = names(text);
        assert!(
            n.contains(&"markup.bold"),
            "**bold** 应产出 markup.bold，实际 {n:?}"
        );
    }

    #[test]
    fn inline_code_span_emits_markup_raw_inline() {
        let text = "para with `code` word\n";
        let n = names(text);
        assert!(
            n.contains(&"markup.raw.inline"),
            "`code` 应产出 markup.raw.inline，实际 {n:?}"
        );
    }

    #[test]
    fn inline_link_emits_markup_link_url_and_text() {
        let text = "see [docs](https://example.com)\n";
        let n = names(text);
        assert!(
            n.contains(&"markup.link.url"),
            "link 目标应产出 markup.link.url，实际 {n:?}"
        );
        assert!(
            n.contains(&"markup.link.text"),
            "link 文本应产出 markup.link.text，实际 {n:?}"
        );
    }

    /// 行内 emphasis 必须落在文本 "italic" 的字节范围里——验证 inline parser
    /// 的局部偏移已被平移回全局坐标。
    #[test]
    fn inline_span_offsets_are_translated_back_to_document_coords() {
        let text = "para with *italic* word\n";
        let (start, end) = span_for(text, "markup.italic").expect("应产出 markup.italic");
        let highlighted = &text[start..end];
        // emphasis 节点可能包含 `*` 分隔符，至少必须把 "italic" 字面值包住。
        assert!(
            highlighted.contains("italic"),
            "markup.italic span 必须覆盖 italic 字面，实际覆盖了 {highlighted:?}"
        );
    }

    /// 标题内部 inline emphasis 必须分段：heading 段 + bold 段 + heading 段
    /// 而不是被整段 markup.heading 吞掉。
    #[test]
    fn heading_with_inline_bold_keeps_bold_segment() {
        let text = "# alpha **beta** gamma\n";
        let spans = collect_names(text);
        // 找 "beta" 在原文的字节位置
        let bold_start = text.find("**beta**").unwrap() + 2; // 跨过开头的两个 *
        let bold_end = bold_start + "beta".len();
        let bold_segment = spans
            .iter()
            .find(|(s, _, n)| *n == "markup.bold" && *s <= bold_start);
        assert!(
            bold_segment.is_some(),
            "heading 内的 **beta** 应保留 markup.bold，实际 spans={spans:?}"
        );
        // heading 仍然占着 "alpha" 段
        let heading_segment = spans.iter().find(|(s, e, n)| {
            *n == "markup.heading"
                && *s <= text.find("alpha").unwrap()
                && *e >= text.find("alpha").unwrap() + 5
        });
        assert!(
            heading_segment.is_some(),
            "heading 的 alpha 段应仍是 markup.heading，实际 spans={spans:?}"
        );
        let _ = bold_end;
    }

    // ============== merge_block_with_inline 单元护栏 ==============

    fn r(s: usize, e: usize) -> TextRange {
        TextRange::new(ByteOffset::new(s), ByteOffset::new(e)).unwrap()
    }

    fn span(name: &'static str) -> HighlightSpan {
        HighlightSpan::new(
            crate::syntax::payload::HighlightName::new(name),
            TokenModifiers::EMPTY,
        )
    }

    #[test]
    fn merge_inline_inside_block_splits_block_three_ways() {
        let block = vec![(r(0, 20), span("markup.heading"))];
        let inline = vec![(r(6, 12), span("markup.bold"))];
        let out = merge_block_with_inline(block, inline);
        let tuples: Vec<(usize, usize, &str)> = out
            .iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str()))
            .collect();
        assert_eq!(
            tuples,
            vec![
                (0, 6, "markup.heading"),
                (6, 12, "markup.bold"),
                (12, 20, "markup.heading"),
            ]
        );
    }

    #[test]
    fn merge_inline_outside_blocks_pass_through() {
        // block 端无覆盖（paragraph 内的纯文本），inline 该原样落地。
        let block: Vec<(TextRange, HighlightSpan)> = Vec::new();
        let inline = vec![
            (r(10, 16), span("markup.italic")),
            (r(20, 26), span("markup.bold")),
        ];
        let out = merge_block_with_inline(block, inline.clone());
        assert_eq!(out, inline);
    }

    #[test]
    fn merge_no_inline_returns_block_unchanged() {
        let block = vec![
            (r(0, 5), span("markup.heading")),
            (r(8, 12), span("markup.raw.block")),
        ];
        let out = merge_block_with_inline(block.clone(), Vec::new());
        assert_eq!(out, block);
    }

    #[test]
    fn normalize_md_inline_maps_canonical_names() {
        assert_eq!(normalize_md_inline("text.literal"), "markup.raw.inline");
        assert_eq!(normalize_md_inline("text.emphasis"), "markup.italic");
        assert_eq!(normalize_md_inline("text.strong"), "markup.bold");
        assert_eq!(normalize_md_inline("text.uri"), "markup.link.url");
        assert_eq!(normalize_md_inline("text.reference"), "markup.link.text");
        assert_eq!(
            normalize_md_inline("punctuation.delimiter"),
            "punctuation.delimiter"
        );
    }

    // ============== 增量护栏（与 common::tests 同形） ==============

    use zom_engine::{Edit as EngineEdit, TextRange as EngineTextRange, Transaction};

    fn apply_replace(buffer: &mut Buffer, start: usize, end: usize, replacement: &str) {
        let range = EngineTextRange::new(ByteOffset::new(start), ByteOffset::new(end)).unwrap();
        let edit = EngineEdit::replace(range, replacement.to_string());
        let tx = Transaction::from_edits(buffer.version(), vec![edit]).unwrap();
        buffer.apply_transaction(tx).unwrap();
    }

    fn pump(worker: &mut MarkdownWorker, buffer: &mut Buffer) {
        let events = buffer.take_pending_events();
        for event in &events {
            worker.on_edit(
                BufferHandle::new(buffer.snapshot()),
                event.changeset(),
                event.new_version(),
            );
        }
    }

    fn drain_latest(sink: &HighlightSink) -> Option<Vec<(TextRange, HighlightSpan)>> {
        let mut latest = None;
        for msg in sink.drain() {
            if let SinkMessage::ReplaceAll { spans, .. } = msg {
                latest = Some(spans);
            }
        }
        latest
    }

    /// 每次都从零 attach 一个 baseline worker，相当于"全量重 parse"。
    fn baseline_spans(buffer: &Buffer) -> Vec<(usize, usize, String)> {
        let mut worker = new_provider();
        let sink = HighlightSink::new();
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
        let spans = drain_latest(&sink).expect("基线必须产出 ReplaceAll");
        spans
            .into_iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str().to_string()))
            .collect()
    }

    fn current_text(buffer: &Buffer) -> String {
        buffer
            .snapshot()
            .slice_byte_range(ByteOffset::ZERO, buffer.snapshot().len_bytes())
            .unwrap()
            .into_text()
            .into_owned()
    }

    /// 几次小编辑（修标题、改 inline 强调、删一行）后，增量 worker 的产物必须
    /// 与每次都从零全量 parse 的 baseline 完全等价。
    #[test]
    fn incremental_matches_full_after_edits() {
        let initial = "# heading\n\nsome *italic* text and `code`.\n\n- one\n- two\n".to_string();
        let mut buffer = Buffer::from_text(initial, BufferConfig::default()).unwrap();
        let mut worker = new_provider();
        let sink = HighlightSink::new();
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
        let _ = drain_latest(&sink);

        // 1) 在标题尾追加一个字
        let step1_pos = "# heading".len();
        apply_replace(&mut buffer, step1_pos, step1_pos, "!");
        pump(&mut worker, &mut buffer);
        let actual = drain_latest(&sink).expect("增量编辑必须产出 ReplaceAll");
        let actual_tuples: Vec<(usize, usize, String)> = actual
            .iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str().to_string()))
            .collect();
        assert_eq!(
            actual_tuples,
            baseline_spans(&buffer),
            "step1：增量 spans 必须等于编辑后的全量 baseline"
        );

        // 2) 把 *italic* 变成 **bold**
        let italic_start = current_text(&buffer).find("*italic*").unwrap();
        apply_replace(
            &mut buffer,
            italic_start,
            italic_start + "*italic*".len(),
            "**bold**",
        );
        pump(&mut worker, &mut buffer);
        let actual = drain_latest(&sink).expect("增量编辑必须产出 ReplaceAll");
        let actual_tuples: Vec<(usize, usize, String)> = actual
            .iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str().to_string()))
            .collect();
        assert_eq!(
            actual_tuples,
            baseline_spans(&buffer),
            "step2：增量 spans 必须等于编辑后的全量 baseline"
        );

        // 3) 删除最后一行
        let two_start = current_text(&buffer).find("- two\n").unwrap();
        apply_replace(&mut buffer, two_start, two_start + "- two\n".len(), "");
        pump(&mut worker, &mut buffer);
        let actual = drain_latest(&sink).expect("增量编辑必须产出 ReplaceAll");
        let actual_tuples: Vec<(usize, usize, String)> = actual
            .iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str().to_string()))
            .collect();
        assert_eq!(
            actual_tuples,
            baseline_spans(&buffer),
            "step3：增量 spans 必须等于编辑后的全量 baseline"
        );

        assert!(
            worker.block_tree.is_some(),
            "块树缓存必须在连续增量编辑后保持有效"
        );
        assert!(worker.last_snapshot.is_some());
    }

    /// 版本断层：worker 漏看一个中间事件时，try_incremental 必须返回 false
    /// 由 on_edit 走 run_full 全量解析，仍能产出与 baseline 一致的 spans。
    #[test]
    fn version_gap_falls_back_to_full() {
        let mut buffer =
            Buffer::from_text("# title\n\npara\n".to_string(), BufferConfig::default()).unwrap();
        let mut worker = new_provider();
        let sink = HighlightSink::new();
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
        let _ = drain_latest(&sink);

        // 第一次编辑事件丢弃，制造版本断层
        let len1 = buffer.snapshot().len_bytes().get();
        apply_replace(&mut buffer, len1, len1, "more\n");
        let _ = buffer.take_pending_events();
        let len2 = buffer.snapshot().len_bytes().get();
        apply_replace(&mut buffer, len2, len2, "tail\n");
        let events = buffer.take_pending_events();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        worker.on_edit(
            BufferHandle::new(buffer.snapshot()),
            event.changeset(),
            event.new_version(),
        );

        let actual = drain_latest(&sink).expect("全量回退仍必须产出 ReplaceAll");
        let actual_tuples: Vec<(usize, usize, String)> = actual
            .iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str().to_string()))
            .collect();
        assert_eq!(actual_tuples, baseline_spans(&buffer));
        assert!(worker.block_tree.is_some(), "run_full 重新填了缓存");
    }

    /// 折叠路径等价性：前 N-1 条走 apply_pending_edit，最后一条走 on_edit，
    /// 最终 spans 必须等于"每条都 on_edit"的顺序应用结果。
    #[test]
    fn apply_pending_edit_then_on_edit_matches_sequential() {
        let initial = "# heading\n\nsome *italic* text and `code`.\n\n- one\n- two\n".to_string();
        // 先在临时 buffer 上跑一遍 step1 取到 step2 的目标位置，使两条路径用同一组 steps
        let steps: Vec<(usize, usize, &str)> = {
            let mut buf = Buffer::from_text(initial.clone(), BufferConfig::default()).unwrap();
            let header_end = "# heading".len();
            apply_replace(&mut buf, header_end, header_end, "!");
            let _ = buf.take_pending_events();
            let italic_start = current_text(&buf).find("*italic*").unwrap();
            vec![
                (header_end, header_end, "!"),
                (italic_start, italic_start + "*italic*".len(), "**bold**"),
            ]
        };

        // 路径 A：折叠
        let coalesced = {
            let mut buffer = Buffer::from_text(initial.clone(), BufferConfig::default()).unwrap();
            let mut worker = new_provider();
            let sink = HighlightSink::new();
            worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
            let _ = sink.drain();

            let last = steps.len() - 1;
            for (i, (start, end, replacement)) in steps.iter().enumerate() {
                apply_replace(&mut buffer, *start, *end, replacement);
                let events = buffer.take_pending_events();
                assert_eq!(events.len(), 1);
                let event = &events[0];
                let handle = BufferHandle::new(buffer.snapshot());
                if i < last {
                    worker.apply_pending_edit(handle, event.changeset(), event.new_version());
                } else {
                    worker.on_edit(handle, event.changeset(), event.new_version());
                }
            }
            drain_latest(&sink).expect("折叠路径最后一条必须产出 ReplaceAll")
        };

        // 路径 B：每条都 on_edit
        let sequential = {
            let mut buffer = Buffer::from_text(initial, BufferConfig::default()).unwrap();
            let mut worker = new_provider();
            let sink = HighlightSink::new();
            worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
            let _ = sink.drain();

            for (start, end, replacement) in &steps {
                apply_replace(&mut buffer, *start, *end, replacement);
                pump(&mut worker, &mut buffer);
            }
            drain_latest(&sink).expect("顺序路径必须产出 ReplaceAll")
        };

        let coalesced_tuples: Vec<(usize, usize, String)> = coalesced
            .iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str().to_string()))
            .collect();
        let sequential_tuples: Vec<(usize, usize, String)> = sequential
            .iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str().to_string()))
            .collect();
        assert_eq!(
            coalesced_tuples, sequential_tuples,
            "折叠路径最终 spans 必须等于每步都 on_edit 的顺序路径"
        );
    }

    /// apply_pending_edit 不能向 sink 投任何消息——折叠路径"省一次 sink push"
    /// 收益的关键不变量。
    #[test]
    fn apply_pending_edit_does_not_push_sink() {
        let mut buffer =
            Buffer::from_text("# title\n".to_string(), BufferConfig::default()).unwrap();
        let mut worker = new_provider();
        let sink = HighlightSink::new();
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
        let _ = sink.drain();

        let len = buffer.snapshot().len_bytes().get();
        apply_replace(&mut buffer, len, len, "more\n");
        let events = buffer.take_pending_events();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        worker.apply_pending_edit(
            BufferHandle::new(buffer.snapshot()),
            event.changeset(),
            event.new_version(),
        );

        let messages = sink.drain();
        assert!(
            messages.is_empty(),
            "apply_pending_edit 不应推 sink：实际推了 {} 条",
            messages.len()
        );
        // 缓存仍在线，下一条 on_edit 可继续走增量。
        assert!(worker.block_tree.is_some());
        assert!(worker.last_snapshot.is_some());
    }

    // ============== viewport-aware 护栏 ==============

    fn split_messages(
        sink: &HighlightSink,
    ) -> (
        Vec<Vec<(TextRange, HighlightSpan)>>,
        Vec<(TextRange, Vec<(TextRange, HighlightSpan)>)>,
    ) {
        let mut all = Vec::new();
        let mut ranges = Vec::new();
        for msg in sink.drain() {
            match msg {
                SinkMessage::ReplaceAll { spans, .. } => all.push(spans),
                SinkMessage::ReplaceRange { range, spans, .. } => ranges.push((range, spans)),
            }
        }
        (all, ranges)
    }

    fn tuples(spans: &[(TextRange, HighlightSpan)]) -> Vec<(usize, usize, String)> {
        spans
            .iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str().to_string()))
            .collect()
    }

    #[test]
    fn run_full_with_hint_emits_anchor_replace_all_plus_replace_range() {
        let text = "# h1\n\n*italic* one\n\n*italic* two\n\n*italic* three\n".to_string();
        let cutoff = text.find("*italic* three").unwrap();
        let viewport = TextRange::new(ByteOffset::new(0), ByteOffset::new(cutoff)).unwrap();

        let buffer = Buffer::from_text(text, BufferConfig::default()).unwrap();
        let mut worker = new_provider();
        let sink = HighlightSink::new();
        worker.set_viewport(Some(viewport));
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());

        let (alls, ranges) = split_messages(&sink);
        assert_eq!(alls.len(), 1, "必须先推一条空 ReplaceAll 锚版本");
        assert!(alls[0].is_empty(), "锚定 ReplaceAll 的 spans 必须为空");
        assert_eq!(ranges.len(), 1, "必须随后推一条 viewport 段 ReplaceRange");
        let (got_range, got_spans) = &ranges[0];
        assert_eq!(*got_range, viewport);
        assert!(!got_spans.is_empty(), "viewport 内至少产出一个 span");
        assert!(
            got_spans.iter().all(|(r, _)| r.end().get() <= cutoff),
            "viewport 段 spans 必须全部落在 viewport 内"
        );
    }

    #[test]
    fn viewport_scoped_spans_equal_full_parse_filtered() {
        let text = "# h1\n\n*italic* one\n\n*italic* two\n\n*italic* three\n".to_string();
        let cutoff = text.find("*italic* three").unwrap();
        let viewport = TextRange::new(ByteOffset::new(0), ByteOffset::new(cutoff)).unwrap();

        let buffer = Buffer::from_text(text, BufferConfig::default()).unwrap();
        let mut worker = new_provider();
        let sink = HighlightSink::new();
        worker.set_viewport(Some(viewport));
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
        let (_, ranges) = split_messages(&sink);
        let mut viewport_tuples = tuples(&ranges[0].1);
        viewport_tuples.sort();

        let mut baseline: Vec<(usize, usize, String)> = baseline_spans(&buffer)
            .into_iter()
            .filter(|(start, end, _)| *start < cutoff && *end <= cutoff)
            .collect();
        baseline.sort();

        assert_eq!(
            viewport_tuples, baseline,
            "viewport-scoped 产物必须等于全量 baseline 在同区间内的子集"
        );
    }

    #[test]
    fn set_viewport_eagerly_emits_replace_range() {
        let text = "# h1\n\n*italic* one\n\n*italic* two\n\n*italic* three\n".to_string();
        let cutoff = text.find("*italic* three").unwrap();
        let viewport = TextRange::new(ByteOffset::new(0), ByteOffset::new(cutoff)).unwrap();

        let buffer = Buffer::from_text(text, BufferConfig::default()).unwrap();
        let mut worker = new_provider();
        let sink = HighlightSink::new();
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
        let (initial_all, initial_ranges) = split_messages(&sink);
        assert_eq!(initial_all.len(), 1, "attach 必须推一份全文 ReplaceAll");
        assert!(initial_ranges.is_empty());

        worker.set_viewport(Some(viewport));
        let (eager_all, eager_ranges) = split_messages(&sink);
        assert!(eager_all.is_empty(), "set_viewport 不应再推 ReplaceAll");
        assert_eq!(
            eager_ranges.len(),
            1,
            "set_viewport 必须立刻推一份 viewport 段 ReplaceRange"
        );
        let (got_range, got_spans) = &eager_ranges[0];
        assert_eq!(*got_range, viewport);
        assert!(!got_spans.is_empty(), "viewport 内必须产出 span");
        assert!(
            got_spans.iter().all(|(r, _)| r.end().get() <= cutoff),
            "eager 重 query 的 spans 必须全部落在 viewport 内"
        );
        assert!(worker.block_tree.is_some());
        assert!(worker.last_snapshot.is_some());
    }

    #[test]
    fn viewport_hint_drives_replace_range_on_edit_and_falls_back_when_cleared() {
        let text = "# h1\n\n*italic* one\n\n*italic* two\n\n*italic* three\n".to_string();
        let cutoff = text.find("*italic* three").unwrap();
        let viewport = TextRange::new(ByteOffset::new(0), ByteOffset::new(cutoff)).unwrap();

        let mut buffer = Buffer::from_text(text, BufferConfig::default()).unwrap();
        let mut worker = new_provider();
        let sink = HighlightSink::new();
        worker.set_viewport(Some(viewport));
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
        let _ = sink.drain();

        let italic_pos = current_text(&buffer).find("*italic*").unwrap();
        apply_replace(
            &mut buffer,
            italic_pos,
            italic_pos + "*italic*".len(),
            "**bold**",
        );
        pump(&mut worker, &mut buffer);

        let (after_all, after_ranges) = split_messages(&sink);
        assert!(
            after_all.is_empty(),
            "viewport 在线时 on_edit 不应推 ReplaceAll"
        );
        assert_eq!(after_ranges.len(), 1, "on_edit 必须以 ReplaceRange 投递");
        let (after_range, after_spans) = &after_ranges[0];
        assert_eq!(*after_range, viewport);
        let lo = after_range.start().get();
        let hi = after_range.end().get();
        assert!(
            after_spans
                .iter()
                .all(|(r, _)| r.start().get() >= lo && r.end().get() <= hi),
            "spans 必须严格落在 ReplaceRange 范围内"
        );

        worker.set_viewport(None);
        let (cleared_all, cleared_ranges) = split_messages(&sink);
        assert!(
            cleared_all.is_empty() && cleared_ranges.is_empty(),
            "清 viewport 本身不应触发产物——回退由下一次 on_edit 驱动"
        );
        let len = buffer.snapshot().len_bytes().get();
        apply_replace(&mut buffer, len, len, "trailing\n");
        pump(&mut worker, &mut buffer);
        let (full_all, full_ranges) = split_messages(&sink);
        assert_eq!(
            full_all.len(),
            1,
            "viewport 清空后 on_edit 必须以 ReplaceAll 回退"
        );
        assert!(full_ranges.is_empty());
        let new_text = current_text(&buffer);
        let third_italic = new_text.find("*italic* three").unwrap();
        let full_spans = &full_all[0];
        assert!(
            full_spans
                .iter()
                .any(|(r, _)| r.start().get() >= third_italic),
            "ReplaceAll 必须覆盖 viewport 之外的 spans"
        );
    }

    #[test]
    fn inline_pass_skips_inline_nodes_outside_viewport() {
        let text = "para *near* one\n\n\n\npara *far* two\n".to_string();
        let cutoff = text.find("para *far*").unwrap();
        let viewport = TextRange::new(ByteOffset::new(0), ByteOffset::new(cutoff)).unwrap();

        let buffer = Buffer::from_text(text.clone(), BufferConfig::default()).unwrap();
        let mut worker = new_provider();
        let sink = HighlightSink::new();
        worker.set_viewport(Some(viewport));
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
        let (_, ranges) = split_messages(&sink);
        let italic_spans: Vec<_> = ranges[0]
            .1
            .iter()
            .filter(|(_, s)| s.name.as_str() == "markup.italic")
            .collect();
        assert!(
            italic_spans.iter().all(|(r, _)| r.start().get() < cutoff),
            "viewport 之外的 *far* 不应被 inline parse；实际 spans={italic_spans:?}"
        );
        let near_start = text.find("*near*").unwrap();
        assert!(
            italic_spans.iter().any(|(r, _)| {
                r.start().get() >= near_start && r.end().get() <= near_start + "*near*".len()
            }),
            "viewport 内的 *near* 必须被高亮，实际 spans={italic_spans:?}"
        );
    }

    #[test]
    fn clip_spans_to_range_clamps_and_drops() {
        let range = TextRange::new(ByteOffset::new(10), ByteOffset::new(20)).unwrap();
        let input = vec![
            (r(0, 5), span("markup.heading")),
            (r(8, 12), span("markup.italic")),
            (r(13, 17), span("markup.bold")),
            (r(18, 25), span("markup.raw.inline")),
            (r(22, 30), span("markup.link.url")),
        ];
        let out = clip_spans_to_range(input, range);
        let got: Vec<(usize, usize, &str)> = out
            .iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (10, 12, "markup.italic"),
                (13, 17, "markup.bold"),
                (18, 20, "markup.raw.inline"),
            ]
        );
    }
}
