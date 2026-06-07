//! Tier 1 provider 的共享机制——所有 tree-sitter Tier 1 provider 都是同一份「OnceLock 配置 + Parser + QueryCursor + sink 槽 + run_full」结构，
//! 差异只在三个常量：`(Language, language_name, HIGHLIGHTS_QUERY)`。
//! 本模块把这一份共同形态抽出来，让每条语言的 provider 文件只剩注册这三个常量。
//!
//! ## 设计要点
//!
//! - **共享 `SharedConfig`**：[`build_shared_config`] 一次性构建，跨同语言多个缓冲区共享一份 `Language` + `Query` + capture-name 索引表。
//! - **capture name 派生不手维护**：
//! 与 `query.capture_names()` 同序、同长的 `Vec<HighlightName>` 在构建时一次性派生；
//! 上游 grammar 升版加 capture时本路径自动跟上（语法高亮手册 §三）。
//! `Box::leak` 把派生 name 提到 `'static`——OnceLock 一次性泄漏 ~20 个短字符串，进程级常量。
//! - **通用 `HighlightWorker`** 实现 [`HighlightProvider`]：调度层拿到的就是它。
//! 每条语言的 `new_provider()` 函数只是包装一次 `LanguageId` 与 `SharedConfig`。
//! - **只处理 highlights**：injections / locals 留空（手册 §十四）；编辑后优先走增量重解析，有 viewport hint 时只投递局部 ReplaceRange。
//!
//! ## 当前解析路径
//!
//! provider 直接使用 `tree_sitter::Parser` + `Query` + `QueryCursor::captures`，自己做嵌套 stack 与「同 node 后到的 pattern 覆盖先到的」语义对齐。
//! 这样可以持久化 `Parser` / `Tree`，并在编辑后优先走增量重解析。
//!
//! 语义护栏：见 `tests::raw_collects_expected_rust_spans`。
//!
//! ## 增量重解析
//!
//! worker 缓存 `Option<Tree>` 与上一次解析用的 `Snapshot`。
//! `on_edit` 时把 `ChangeSet::edits()`（旧坐标）+ 旧 / 新 Snapshot 翻译成 `Vec<InputEdit>`，逐条调 `Tree::edit`，再走 `Parser::parse_with_options` 喂流式 rope chunks，并把旧 Tree 作为 `Some(&old)` 传入。
//!
//! 三类失败路径都收口到全量重解析 + 复位 Tree（覆盖在 `run_full` 里）：
//! 1. 无缓存（首次 attach / 上一轮回退过）；
//! 2. 任一条 InputEdit 翻译失败（如旧 offset 越界、坐标解码出错）；
//! 3. `parser.parse_with_options` 返回 `None`（grammar ABI / 内部错误）。
//!
//! 等价性护栏：`tests::incremental_matches_full_after_edit` 把同样几次小编辑分别用「增量 worker」与「每次都从零 parse 的 baseline」跑一遍，断言 spans 完全一致。
//!
//! 有 viewport hint 时，query 限制在 viewport ± 缓冲区，并以 ReplaceRange 投递局部 spans；
//! 无 hint 时走全文 ReplaceAll。
//!
//! ## sink 缓存
//!
//! trait 只在 `attach` 时给 sink；`on_edit` 不再传。
//! provider 内部缓存 sink，编辑或 viewport hint 改变时复用。
//! sink 是轻量 clone 的 Arc，本就为这种场景设计。

use std::{fmt::Debug, sync::Arc};

use tree_sitter::{
    InputEdit, Language, Node, Parser, Point, Query, QueryCursor, QueryError, StreamingIterator,
    TextProvider, Tree,
};

use crate::syntax::LanguageId;
use crate::syntax::payload::{HighlightName, HighlightSpan, TokenModifiers};
use crate::syntax::provider::{BufferHandle, HighlightProvider};
use crate::syntax::tree::{BufferSyntaxTree, BufferSyntaxTreeSlot};
use zom_engine::{BufferVersion, ByteOffset, ChangeSet, Snapshot, TextRange};

/// 一门语言已 build 好的高亮配置：
/// `tree_sitter::Query` 加上派生的「capture index → [`HighlightName`]」索引表与原始 `Language`。
pub(crate) struct SharedConfig {
    pub(crate) language: Language,
    pub(crate) query: Query,
    /// 与 `query.capture_names()` 同序、同长。
    pub(crate) lookup: Vec<HighlightName>,
}

/// 给一门语言构建一份 [`SharedConfig`]。
///
/// 失败仅发生在 query 语法错误或 ABI 不匹配——静态资源问题，发版前必被测试覆盖（语法高亮手册 §十二「降级与边界条件」）。
/// 每条语言的 provider 文件在 `OnceLock` 内调一次。
pub(crate) fn build_shared_config(
    language: Language,
    highlights_query: &'static str,
) -> Result<SharedConfig, QueryError> {
    build_shared_config_with_normalize(language, highlights_query, normalize_highlight_name)
}

/// 同 [`build_shared_config`]，但允许传入自定义 capture-name 归一化函数。
///
/// **当前唯一调用方是 markdown 的 inline grammar**（[`super::markdown`]） —— inline grammar 的 `text.literal` 指 `code_span`，
/// 要归一到 `markup.raw.inline`，而 block grammar 的 `text.literal` 指 fenced/indented code block，归一到 `markup.raw.block`。
/// 两者冲突，单一全局 [`normalize_highlight_name`] 表达不下。
/// 其余语言走默认归一化即可，不必各自重复。
pub(crate) fn build_shared_config_with_normalize(
    language: Language,
    highlights_query: &'static str,
    normalize: fn(&str) -> &str,
) -> Result<SharedConfig, QueryError> {
    let query = Query::new(&language, highlights_query)?;
    let lookup: Vec<HighlightName> = query
        .capture_names()
        .iter()
        .map(|name| {
            let normalized = normalize(name);
            HighlightName::new(Box::leak(normalized.to_string().into_boxed_str()))
        })
        .collect();
    Ok(SharedConfig {
        language,
        query,
        lookup,
    })
}

/// 把不同 tree-sitter query 生态里的 capture 方言归一到主题使用的 canonical name。
///
/// 当前只覆盖已经观察到的 nvim-treesitter / tree-sitter-md `text.*` 命名；
/// 其余 capture 保持原名，让 Helix/Zed 风格 query 可以零成本透传。
fn normalize_highlight_name(name: &str) -> &str {
    match name {
        "text.title" => "markup.heading",
        "text.literal" => "markup.raw.block",
        "text.uri" => "markup.link.url",
        "text.reference" => "markup.link.text",
        other => other,
    }
}

/// 通用 Tier 1 provider 实现。
///
/// 每个缓冲区一份 `HighlightWorker`，但内部 `Arc<SharedConfig>` 跨缓冲区共享。
/// 调度层只看 [`HighlightProvider`] trait，并不知道 worker 装的是哪门语言——这正是「产出者形态统一、差异封装在内部」的实现兑现。
///
/// Phase 3 后形态：worker 只负责 parse + tree 缓存，**不产 spans**。
/// spans 由 paint 阶段从共享 [`BufferSyntaxTree`] 现查；worker 通过 `export_syntax_tree` 把内部 `(tree, snapshot)` 写到共享 slot 即可。
pub struct HighlightWorker {
    language_id: LanguageId,
    config: Arc<SharedConfig>,
    parser: Parser,
    /// 上一次解析出的 Tree。
    /// `None` 表示尚未首次解析或上一轮失败（下次 `on_edit` 走全量重解析把这两个槽都填回去）。
    tree: Option<Tree>,
    /// 与 `tree` 对应的 Snapshot：增量路径计算 InputEdit **旧端** Point 时需要它（新端 Point 用 on_edit 收到的新 snapshot）。
    /// Snapshot 内部是 Arc 共享 rope，常驻一份开销可忽略。
    last_snapshot: Option<Snapshot>,
}

impl Debug for HighlightWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HighlightWorker")
            .field("language", &self.language_id)
            .finish_non_exhaustive()
    }
}

impl HighlightWorker {
    pub(crate) fn new(language_id: LanguageId, config: Arc<SharedConfig>) -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&config.language)
            .expect("tree-sitter set_language 失败：语法 ABI 不匹配，发版前必须被测试覆盖");
        Self {
            language_id,
            config,
            parser,
            tree: None,
            last_snapshot: None,
        }
    }

    /// 全量解析缓冲区当前 snapshot，把新 Tree 与 Snapshot 落进 worker 缓存。
    ///
    /// 缓存的 `tree` 不能跨 snapshot 复用而不调用 `Tree::edit` —— 这条规则由增量路径的 `try_incremental` 保证。Phase 3 后不再 query / 推 sink；
    /// spans 由 paint 阶段从共享 [`BufferSyntaxTree`] 现查。
    fn run_full(&mut self, buffer: &BufferHandle) {
        let snapshot = buffer.snapshot();
        let text =
            match snapshot.slice_byte_range(zom_engine::ByteOffset::ZERO, snapshot.len_bytes()) {
                Ok(text) => text.into_text().into_owned(),
                Err(_) => {
                    self.tree = None;
                    self.last_snapshot = None;
                    return;
                }
            };
        let bytes = text.as_bytes();

        let Some(tree) = self.parser.parse(bytes, None) else {
            self.tree = None;
            self.last_snapshot = None;
            return;
        };

        self.tree = Some(tree);
        self.last_snapshot = Some(snapshot);
    }

    /// 尝试增量重解析；返回 `false` 表示需要调用方走全量解析。
    ///
    /// 失败路径：缓存缺失 / 版本不连续 / InputEdit 翻译失败 / `parse_with_options` 返回 `None`。
    /// 任一失败都把 `tree`/`last_snapshot` 槽清空让调用方走 `run_full`。
    fn try_incremental(
        &mut self,
        buffer: &BufferHandle,
        change: &ChangeSet,
        new_version: BufferVersion,
    ) -> bool {
        let (Some(old_snapshot), Some(mut tree)) = (self.last_snapshot.take(), self.tree.take())
        else {
            return false;
        };
        if old_snapshot.version().get().saturating_add(1) != new_version.get() {
            return false;
        }
        let new_snapshot = buffer.snapshot();

        let input_edits = match translate_edits(change, &old_snapshot, &new_snapshot) {
            Some(edits) => edits,
            None => return false,
        };
        for ie in &input_edits {
            tree.edit(ie);
        }

        let new_tree = {
            let snap_ref = &new_snapshot;
            let total = new_snapshot.len_bytes().get();
            let mut cb = |byte_offset: usize, _point: Point| -> &[u8] {
                if byte_offset >= total {
                    return b"";
                }
                match snap_ref.chunk_at_byte(ByteOffset::new(byte_offset)) {
                    Ok((chunk, chunk_start)) => {
                        let local = byte_offset - chunk_start.get();
                        &chunk.as_bytes()[local..]
                    }
                    Err(_) => b"",
                }
            };
            self.parser.parse_with_options(&mut cb, Some(&tree), None)
        };

        let Some(new_tree) = new_tree else {
            return false;
        };

        self.tree = Some(new_tree);
        self.last_snapshot = Some(new_snapshot);
        true
    }
}

/// 把 `QueryCursor::set_byte_range` 回到全文范围——viewport-scoped query 完成后立刻 reset，保证后续 run_full 等全文路径不被上一次约束截断。
///
/// tree-sitter 内部把 byte range 端点 cast 成 u32；`u32::MAX as usize` 是它支持的最大值。
pub(crate) fn reset_cursor_range(cursor: &mut QueryCursor) {
    cursor.set_byte_range(0..(u32::MAX as usize));
}

/// 把 `ChangeSet`（旧坐标 edit 列表）翻译为 tree-sitter `InputEdit` 列表。
///
/// 每条 InputEdit 的旧端 Point 用 `old_snapshot.byte_to_point` 解码，新端 Point 用 `new_snapshot.byte_to_point` 解码，这两侧坐标系不同源（一个是旧文本、一个是新文本），不能混用同一份 snapshot；详见改造方案 §3.4。
///
/// 任何一条 edit 解码失败（越界 / 非字符边界）就返回 `None`，由调用方全量重解析。
/// Edit 列表已经过 `EditList::new` 排序且不重叠，按顺序逐条 `Tree::edit` 即可保证 tree-sitter 看到的内部坐标连续推进。
pub(crate) fn translate_edits(
    change: &ChangeSet,
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
) -> Option<Vec<InputEdit>> {
    let edits = change.edits();
    let mut out = Vec::with_capacity(edits.len());
    let mut shift: isize = 0;
    for edit in edits {
        let range = edit.range();
        let start_byte = range.start().get();
        let old_end_byte = range.end().get();
        let new_end_byte = start_byte
            .checked_add(edit.replacement().len())
            .filter(|v| *v <= isize::MAX as usize)?;

        // 旧坐标侧：直接用 old_snapshot。
        let (start_line, start_col) = old_snapshot
            .byte_to_point(ByteOffset::new(start_byte))
            .ok()?;
        let (old_end_line, old_end_col) = old_snapshot
            .byte_to_point(ByteOffset::new(old_end_byte))
            .ok()?;

        // 新坐标侧：把旧 start_byte 通过当前累计位移映射到新坐标，再到 new_snapshot 查 Point。
        let shifted_start = (start_byte as isize) + shift;
        if shifted_start < 0 {
            return None;
        }
        let new_start_byte_in_new = shifted_start as usize;
        let new_end_byte_in_new = new_start_byte_in_new + edit.replacement().len();
        if new_end_byte_in_new > new_snapshot.len_bytes().get() {
            return None;
        }
        let (new_end_line, new_end_col) = new_snapshot
            .byte_to_point(ByteOffset::new(new_end_byte_in_new))
            .ok()?;

        out.push(InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_position: Point::new(start_line.get(), start_col),
            old_end_position: Point::new(old_end_line.get(), old_end_col),
            new_end_position: Point::new(new_end_line.get(), new_end_col),
        });

        shift += edit.replacement().len() as isize - (old_end_byte - start_byte) as isize;
    }
    Some(out)
}

/// `tree_sitter::TextProvider` 实现：predicate 评估时按需借出 rope chunks，不复制节点字节。
///
/// 用于 `QueryCursor::captures` 的 predicate 路径（rust grammar 用了 `#match?`做大写常量 / 类型识别），让 worker 不必事先物化全文。
/// tree-sitter 把 chunk 流概念吸进 predicate 求值器内部，跨 chunk 拼接由它自己处理；
/// 调用方按任意 UTF-8 字节边界续读即可。
///
/// **零拷贝**：每个 `text(node)` 调用返回的迭代器只持 `&'snap Snapshot` 引用，逐 chunk 借出 `&'snap [u8]`，不再为每个 predicate 节点 alloc 一个 `Vec<u8>`——16 MiB rust 一档单键 viewport-scoped query 内 predicate 触发频次按"节点遍历数"线性增长，这里省下来的 alloc/copy 是大头。
pub(crate) struct SnapshotTextProvider<'snap> {
    pub(crate) snapshot: &'snap Snapshot,
}

/// 沿 rope 边界懒迭代的 chunk 序列；上界由节点 `end_byte` 收口。
pub(crate) struct SnapshotChunkIter<'snap> {
    snapshot: &'snap Snapshot,
    cur: usize,
    end: usize,
}

impl<'snap> Iterator for SnapshotChunkIter<'snap> {
    type Item = &'snap [u8];
    fn next(&mut self) -> Option<Self::Item> {
        if self.cur >= self.end {
            return None;
        }
        let (chunk, chunk_start) = self
            .snapshot
            .chunk_at_byte(ByteOffset::new(self.cur))
            .ok()?;
        let local = self.cur - chunk_start.get();
        let chunk_bytes = chunk.as_bytes();
        if local >= chunk_bytes.len() {
            return None;
        }
        let avail = &chunk_bytes[local..];
        let take = avail.len().min(self.end - self.cur);
        if take == 0 {
            return None;
        }
        self.cur += take;
        Some(&avail[..take])
    }
}

impl<'snap> TextProvider<&'snap [u8]> for SnapshotTextProvider<'snap> {
    type I = SnapshotChunkIter<'snap>;
    fn text(&mut self, node: Node<'_>) -> Self::I {
        SnapshotChunkIter {
            snapshot: self.snapshot,
            cur: node.start_byte(),
            end: node.end_byte(),
        }
    }
}

/// 把 `QueryCursor::captures` 的事件流转成非重叠 `(range, name)` span 列表。
///
/// 语义沿用 tree-sitter 官方 highlighter 的事件模型，只去除我们当前不消费的 injections / locals 两条支路：
///
/// 1. **同 node 覆盖**：`captures` 按 source order 流出；
/// 同一 node 上多个 pattern 依次出现时，**后到的 pattern 胜出**——下游的 `(node, pattern_b)` 覆盖先到的 `(node, pattern_a)`。
/// 这条规则让 grammar 通过把更具体的 query 写在更下方实现优先级。
/// 2. **嵌套 inner-wins**：一个 capture 的 byte range 完全落在前一个未结束 capture 范围内时，新 capture 入栈作为当前生效高亮，旧 capture 在 inner 结束后**继续**生效——产出 `[outer_prefix, inner, outer_suffix]` 三段 span。
/// 3. **栈空段不产 span**：未被任何 capture 覆盖的字节默认走主题前景色，span 表不记录这种"无高亮"段落。
///
/// 实现用「延迟一拍处理 pending capture」模式：每收到新 capture 时把上一条 pending 与之比对——同 node 就替换 pending、不同 node 就先 finalize 上一条再排上新的。
/// `StreamingIterator` 不能 peek，这种延迟一拍写法是适配它的最简方式。
pub(crate) fn collect_spans<P, I>(
    config: &SharedConfig,
    cursor: &mut QueryCursor,
    tree: &Tree,
    source: P,
) -> Vec<(TextRange, HighlightSpan)>
where
    P: TextProvider<I>,
    I: AsRef<[u8]>,
{
    let mut spans: Vec<(TextRange, HighlightSpan)> = Vec::new();
    let mut active_stack: Vec<ActiveScope> = Vec::new();
    let mut last_emit_byte: usize = 0;
    let mut pending: Option<PendingCapture> = None;

    let mut captures = cursor.captures(&config.query, tree.root_node(), source);
    while let Some((m, idx)) = captures.next() {
        let capture = m.captures[*idx];
        let current = PendingCapture {
            node_id: capture.node.id(),
            start_byte: capture.node.start_byte(),
            end_byte: capture.node.end_byte(),
            capture_index: capture.index as usize,
        };
        match pending.take() {
            Some(prev) if prev.node_id == current.node_id => {
                // 同 node：后到的 pattern 胜出，丢弃 prev、保留 current 为新 pending。
                pending = Some(current);
            }
            Some(prev) => {
                finalize_capture(
                    prev,
                    config,
                    &mut active_stack,
                    &mut spans,
                    &mut last_emit_byte,
                );
                pending = Some(current);
            }
            None => {
                pending = Some(current);
            }
        }
    }
    if let Some(prev) = pending {
        finalize_capture(
            prev,
            config,
            &mut active_stack,
            &mut spans,
            &mut last_emit_byte,
        );
    }

    // 流结束后，剩余栈逐层退出，把"最后一段 inner 之后到 outer 末尾"补齐。
    while let Some(top) = active_stack.pop() {
        emit_span(
            last_emit_byte,
            top.end_byte,
            top.name_index,
            config,
            &mut spans,
        );
        last_emit_byte = top.end_byte;
    }
    spans
}

struct PendingCapture {
    node_id: usize,
    start_byte: usize,
    end_byte: usize,
    capture_index: usize,
}

struct ActiveScope {
    end_byte: usize,
    name_index: usize,
}

fn finalize_capture(
    cap: PendingCapture,
    config: &SharedConfig,
    active_stack: &mut Vec<ActiveScope>,
    spans: &mut Vec<(TextRange, HighlightSpan)>,
    last_emit_byte: &mut usize,
) {
    // 关掉所有在本 capture 起点之前已结束的栈层，每层都补一段 span（如果有内容）。
    while let Some(top) = active_stack.last() {
        if top.end_byte <= cap.start_byte {
            emit_span(*last_emit_byte, top.end_byte, top.name_index, config, spans);
            *last_emit_byte = top.end_byte;
            active_stack.pop();
        } else {
            break;
        }
    }
    // 栈非空：从 last_emit 到 cap.start 还属于当前栈顶 capture 的范围。
    // 先把这段 outer-prefix span 落下。
    if let Some(top) = active_stack.last() {
        emit_span(
            *last_emit_byte,
            cap.start_byte,
            top.name_index,
            config,
            spans,
        );
    }
    *last_emit_byte = cap.start_byte;
    // 入栈：cap 成为新的"当前生效"高亮，直到它结束或被更内层 cap 覆盖。
    active_stack.push(ActiveScope {
        end_byte: cap.end_byte,
        name_index: cap.capture_index,
    });
}

fn emit_span(
    start: usize,
    end: usize,
    name_index: usize,
    config: &SharedConfig,
    spans: &mut Vec<(TextRange, HighlightSpan)>,
) {
    if start >= end {
        return;
    }
    let Some(name) = config.lookup.get(name_index).copied() else {
        return;
    };
    let Ok(range) = TextRange::new(ByteOffset::new(start), ByteOffset::new(end)) else {
        return;
    };
    spans.push((range, HighlightSpan::new(name, TokenModifiers::EMPTY)));
}

impl HighlightProvider for HighlightWorker {
    fn language(&self) -> LanguageId {
        self.language_id
    }

    fn attach(&mut self, buffer: BufferHandle) {
        self.run_full(&buffer);
    }

    fn on_edit(&mut self, buffer: BufferHandle, change: &ChangeSet, version: BufferVersion) {
        // 多事件批量 pump 的中间事件：BufferHandle 已是该批最终状态，与本事件的 `new_version` 不一致。
        // 此时既不能增量（缺中间 snapshot），也不必全量（终态事件马上会再驱动一次），直接无操作。
        if buffer.version() != version {
            return;
        }
        if !self.try_incremental(&buffer, change, version) {
            self.run_full(&buffer);
        }
    }

    fn detach(&mut self) {
        self.tree = None;
        self.last_snapshot = None;
    }

    /// 把 worker 内部最新的 `tree` + `snapshot` 写到共享 slot，让主线程 paint 端能按 viewport 现查 Query。
    /// `store_if_newer` 保证不会把过期 reparse 结果盖到主线程 `tree.edit` 已经推进过的更新版本上。
    ///
    /// 任一槽位缺失（首次 attach 失败 / 上一轮回退过）直接返回，让 slot 维持上次值。
    fn export_syntax_tree(&self, slot: &BufferSyntaxTreeSlot) {
        let (Some(tree), Some(snapshot)) = (self.tree.as_ref(), self.last_snapshot.as_ref()) else {
            return;
        };
        let version = snapshot.version();
        slot.store_if_newer(BufferSyntaxTree::new(
            self.config.clone(),
            tree.clone(),
            snapshot.clone(),
            version,
        ));
    }
}

// =============================================================================
// 单测用辅助
// =============================================================================
#[cfg(test)]
pub(crate) fn assert_lookup_matches_capture_names(config: &SharedConfig) {
    // 派生路径自洽：lookup 与 query.capture_names() 同序、同长，且每项等于 capture name 归一化后的 canonical name。
    // 这条断言保证未来任何修改不会让两侧错位。
    let capture_names: Vec<&str> = config.query.capture_names().iter().copied().collect();
    assert_eq!(config.lookup.len(), capture_names.len());
    for (i, name) in capture_names.iter().enumerate() {
        assert_eq!(config.lookup[i].as_str(), normalize_highlight_name(name));
    }
}

/// 给一门语言的 provider 跑「装上 → 喂样本 → 至少 query 出一个 span」的烟雾测试。
///
/// 不断言具体 capture name——name 是 grammar 内部细节，对未来升级太脆；只要
/// 证明 provider 接进调度链路、tree 落到 slot、paint 端 query 能出 spans。
#[cfg(test)]
pub(crate) fn smoke_test_provider<F>(language_id: LanguageId, sample: &str, make: F)
where
    F: FnOnce() -> HighlightWorker,
{
    use crate::BufferId;
    use crate::syntax::{BufferSyntax, SyntaxQueryCursor, SyntaxWorkerHandle};
    use std::sync::Arc;
    use zom_engine::{Buffer, BufferConfig, ByteOffset, TextRange};

    let buffer = Buffer::from_text(sample.to_string(), BufferConfig::default()).unwrap();
    let provider: Box<dyn HighlightProvider> = Box::new(make());
    let worker = Arc::new(SyntaxWorkerHandle::spawn());
    let syntax = BufferSyntax::attach(
        BufferId::from_raw(1),
        language_id,
        provider,
        &buffer,
        worker.clone(),
    );
    worker.wait_for_idle();
    let tree = syntax
        .tree_slot()
        .load()
        .expect("attach 完成后 slot 必须有 tree");
    let viewport =
        TextRange::new(ByteOffset::ZERO, buffer.snapshot().len_bytes()).expect("空文档边界");
    let mut cursor = SyntaxQueryCursor::new();
    let spans = tree.query_viewport(viewport, &mut cursor);
    assert!(!spans.is_empty(), "provider 应至少为样本文本产出一个 span");
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::OnceLock;
    use zom_engine::{
        Buffer, BufferConfig, Edit as EngineEdit, TextRange as EngineTextRange, Transaction,
    };

    #[test]
    fn raw_collects_expected_rust_spans() {
        let sample = "pub fn answer() -> i32 {\n    let value = 42;\n    value\n}\n";
        let raw_spans = collect_via_raw(sample);

        assert_eq!(
            raw_spans,
            vec![
                (0, 3, "keyword"),
                (4, 6, "keyword"),
                (7, 13, "function"),
                (13, 14, "punctuation.bracket"),
                (14, 15, "punctuation.bracket"),
                (19, 22, "type.builtin"),
                (23, 24, "punctuation.bracket"),
                (29, 32, "keyword"),
                (41, 43, "constant.builtin"),
                (43, 44, "punctuation.delimiter"),
                (55, 56, "punctuation.bracket"),
            ]
        );
    }

    #[test]
    fn normalizes_common_capture_dialects() {
        assert_eq!(normalize_highlight_name("text.title"), "markup.heading");
        assert_eq!(normalize_highlight_name("text.literal"), "markup.raw.block");
        assert_eq!(normalize_highlight_name("text.uri"), "markup.link.url");
        assert_eq!(
            normalize_highlight_name("text.reference"),
            "markup.link.text"
        );
        assert_eq!(
            normalize_highlight_name("keyword.control"),
            "keyword.control"
        );
    }

    fn collect_via_raw(source: &str) -> Vec<(usize, usize, &'static str)> {
        let language: Language = tree_sitter_rust::LANGUAGE.into();
        let query = Query::new(&language, tree_sitter_rust::HIGHLIGHTS_QUERY).unwrap();
        let lookup: Vec<HighlightName> = query
            .capture_names()
            .iter()
            .map(|name| HighlightName::new(Box::leak((*name).to_string().into_boxed_str())))
            .collect();
        let config = SharedConfig {
            language,
            query,
            lookup,
        };
        let mut parser = Parser::new();
        parser.set_language(&config.language).unwrap();
        let tree = parser.parse(source.as_bytes(), None).unwrap();
        let mut cursor = QueryCursor::new();
        let spans = collect_spans(&config, &mut cursor, &tree, source.as_bytes());
        spans
            .into_iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str()))
            .collect()
    }

    // ============== 增量护栏 ==============

    fn rust_config() -> Arc<SharedConfig> {
        static CACHE: OnceLock<Arc<SharedConfig>> = OnceLock::new();
        CACHE
            .get_or_init(|| {
                let language: Language = tree_sitter_rust::LANGUAGE.into();
                Arc::new(build_shared_config(language, tree_sitter_rust::HIGHLIGHTS_QUERY).unwrap())
            })
            .clone()
    }

    fn apply_replace(buffer: &mut Buffer, start: usize, end: usize, replacement: &str) {
        let range = EngineTextRange::new(ByteOffset::new(start), ByteOffset::new(end)).unwrap();
        let edit = EngineEdit::replace(range, replacement.to_string());
        let tx = Transaction::from_edits(buffer.version(), vec![edit]).unwrap();
        buffer.apply_transaction(tx).unwrap();
    }

    fn pump(worker: &mut HighlightWorker, buffer: &mut Buffer) {
        let events = buffer.take_pending_events();
        for event in &events {
            worker.on_edit(
                BufferHandle::new(buffer.snapshot()),
                event.changeset(),
                event.new_version(),
            );
        }
    }

    /// 把 worker 当前内部 `(tree, snapshot)` 用全文 query 拍出 spans。
    fn worker_full_spans(worker: &HighlightWorker) -> Vec<(usize, usize, String)> {
        let tree = worker.tree.as_ref().expect("worker 必须有 tree");
        let snapshot = worker
            .last_snapshot
            .as_ref()
            .expect("worker 必须有 snapshot");
        let bytes = snapshot
            .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
            .expect("全文切片")
            .into_text()
            .into_owned();
        let mut cursor = QueryCursor::new();
        let spans = collect_spans(&worker.config, &mut cursor, tree, bytes.as_bytes());
        spans
            .into_iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str().to_string()))
            .collect()
    }

    /// 从零 attach 一个 baseline worker，等价于「全量重 parse」结果。
    fn baseline_spans(buffer: &Buffer) -> Vec<(usize, usize, String)> {
        let mut worker = HighlightWorker::new(LanguageId::new("rust"), rust_config());
        worker.attach(BufferHandle::new(buffer.snapshot()));
        worker_full_spans(&worker)
    }

    #[test]
    fn incremental_matches_full_after_edits() {
        // 几次小编辑后，增量 worker 的内部 tree 跑出的 spans 必须与每次都全量 parse
        // 的 baseline 完全等价 —— 计划 §Phase 4 "edit-frame paint == reparse-frame paint"
        // 的核心不变量。
        let initial = "pub fn answer() -> i32 {\n    let value = 42;\n    value\n}\n".to_string();
        let mut buffer = Buffer::from_text(initial, BufferConfig::default()).unwrap();
        let mut worker = HighlightWorker::new(LanguageId::new("rust"), rust_config());
        worker.attach(BufferHandle::new(buffer.snapshot()));

        let steps: &[(usize, usize, &str)] =
            &[(21, 22, "6"), (29, 29, "// hi\n    "), (66, 67, "")];
        for (start, end, replacement) in steps {
            apply_replace(&mut buffer, *start, *end, replacement);
            pump(&mut worker, &mut buffer);
            let actual = worker_full_spans(&worker);
            let expected = baseline_spans(&buffer);
            assert_eq!(
                actual, expected,
                "增量 spans 必须等于编辑后的全量解析基线（{start}..{end} <- {replacement:?}）"
            );
        }

        assert!(worker.tree.is_some());
        assert!(worker.last_snapshot.is_some());
    }

    #[test]
    fn version_gap_falls_back_to_full() {
        // 模拟 worker 错过一个中间事件：缓存版本不等于 new_version - 1 时，
        // try_incremental 必须返回 false，由 on_edit 走 run_full 全量解析。
        let mut buffer =
            Buffer::from_text("fn a() {}\n".to_string(), BufferConfig::default()).unwrap();
        let mut worker = HighlightWorker::new(LanguageId::new("rust"), rust_config());
        worker.attach(BufferHandle::new(buffer.snapshot()));

        apply_replace(&mut buffer, 9, 9, " ");
        let _ = buffer.take_pending_events();
        apply_replace(&mut buffer, 10, 10, "// tail\n");
        let events = buffer.take_pending_events();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        worker.on_edit(
            BufferHandle::new(buffer.snapshot()),
            event.changeset(),
            event.new_version(),
        );

        let actual = worker_full_spans(&worker);
        let expected = baseline_spans(&buffer);
        assert_eq!(actual, expected, "版本跳跃时全量路径必须产出完整 spans");
        assert!(worker.tree.is_some());
    }
}
