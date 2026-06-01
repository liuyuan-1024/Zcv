//! Tier 1 provider 的共享机制——所有 tree-sitter Tier 1 provider 都是同一份
//! 「OnceLock 配置 + Parser + QueryCursor + sink 槽 + run_full」结构，
//! 差异只在三个常量：`(Language, language_name, HIGHLIGHTS_QUERY)`。本模块把
//! 这一份共同形态抽出来，让每条语言的 provider 文件只剩注册这三个常量。
//!
//! ## 设计要点
//!
//! - **共享 `SharedConfig`**：[`build_shared_config`] 一次性构建，跨同语言
//!   多个缓冲区共享一份 `Language` + `Query` + capture-name 索引表。
//! - **capture name 派生不手维护**：与 `query.capture_names()` 同序、同长
//!   的 `Vec<HighlightName>` 在构建时一次性派生；上游 grammar 升版加 capture
//!   时本路径自动跟上（语法高亮手册 §三）。`Box::leak` 把派生 name 提到
//!   `'static`——OnceLock 一次性泄漏 ~20 个短字符串，进程级常量。
//! - **通用 `HighlightWorker`** 实现 [`HighlightProvider`]：调度层拿到的就是
//!   它。每条语言的 `new_provider()` 函数只是包装一次 `LanguageId` 与
//!   `SharedConfig`。
//! - **只处理 highlights**：injections / locals 留空（手册 §十四）；编辑后优先走
//!   增量重解析，有 viewport hint 时只投递局部 ReplaceRange。
//!
//! ## 当前解析路径
//!
//! provider 直接使用 `tree_sitter::Parser` + `Query` + `QueryCursor::captures`，
//! 自己做嵌套 stack 与「同 node 后到的 pattern 覆盖先到的」语义对齐。这样可以
//! 持久化 `Parser` / `Tree`，并在编辑后优先走增量重解析。
//!
//! 语义护栏：见 `tests::raw_collects_expected_rust_spans`。
//!
//! ## 增量重解析
//!
//! worker 缓存 `Option<Tree>` 与上一次解析用的 `Snapshot`。`on_edit` 时把
//! `ChangeSet::edits()`（旧坐标）+ 旧 / 新 Snapshot 翻译成 `Vec<InputEdit>`，
//! 逐条调 `Tree::edit`，再走 `Parser::parse_with_options` 喂流式 rope chunks，
//! 并把旧 Tree 作为 `Some(&old)` 传入。
//!
//! 三类失败路径都收口到全量重解析 + 复位 Tree（覆盖在 `run_full` 里）：
//! 1. 无缓存（首次 attach / 上一轮回退过）；
//! 2. 任一条 InputEdit 翻译失败（如旧 offset 越界、坐标解码出错）；
//! 3. `parser.parse_with_options` 返回 `None`（grammar ABI / 内部错误）。
//!
//! 等价性护栏：`tests::incremental_matches_full_after_edit` 把同样几次小编辑
//! 分别用「增量 worker」与「每次都从零 parse 的 baseline」跑一遍，断言 spans
//! 完全一致。
//!
//! 有 viewport hint 时，query 限制在 viewport ± 缓冲区，并以 ReplaceRange
//! 投递局部 spans；无 hint 时走全文 ReplaceAll。
//!
//! ## sink 缓存
//!
//! trait 只在 `attach` 时给 sink；`on_edit` 不再传。provider 内部缓存 sink，
//! 编辑或 viewport hint 改变时复用。sink 是轻量 clone 的 Arc，本就为这种场景设计。

use std::{fmt::Debug, sync::Arc};

use tree_sitter::{
    InputEdit, Language, Node, Parser, Point, Query, QueryCursor, QueryError, StreamingIterator,
    TextProvider, Tree,
};

use crate::syntax::LanguageId;
use crate::syntax::payload::{HighlightName, HighlightSpan, TokenModifiers};
use crate::syntax::provider::{BufferHandle, HighlightProvider};
use crate::syntax::sink::HighlightSink;
use zom_engine::{BufferVersion, ByteOffset, ChangeSet, Snapshot, TextRange};

/// 一门语言已 build 好的高亮配置：`tree_sitter::Query` 加上派生的
/// 「capture index → [`HighlightName`]」索引表与原始 `Language`。
pub(crate) struct SharedConfig {
    pub(crate) language: Language,
    pub(crate) query: Query,
    /// 与 `query.capture_names()` 同序、同长。
    pub(crate) lookup: Vec<HighlightName>,
}

/// 给一门语言构建一份 [`SharedConfig`]。
///
/// 失败仅发生在 query 语法错误或 ABI 不匹配——静态资源问题，发版前必被
/// 测试覆盖（语法高亮手册 §十二「降级与边界条件」）。每条语言的 provider
/// 文件在 `OnceLock` 内调一次。
pub(crate) fn build_shared_config(
    language: Language,
    highlights_query: &'static str,
) -> Result<SharedConfig, QueryError> {
    let query = Query::new(&language, highlights_query)?;
    let lookup: Vec<HighlightName> = query
        .capture_names()
        .iter()
        .map(|name| {
            let normalized = normalize_highlight_name(name);
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
/// 当前只覆盖已经观察到的 nvim-treesitter / tree-sitter-md `text.*` 命名；其余
/// capture 保持原名，让 Helix/Zed 风格 query 可以零成本透传。
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
/// 每个缓冲区一份 `HighlightWorker`，但内部 `Arc<SharedConfig>` 跨缓冲区
/// 共享。调度层只看 [`HighlightProvider`] trait，并不知道 worker 装的是哪门
/// 语言——这正是「产出者形态统一、差异封装在内部」的实现兑现。
pub struct HighlightWorker {
    language_id: LanguageId,
    config: Arc<SharedConfig>,
    parser: Parser,
    cursor: QueryCursor,
    /// attach 时缓存进来；detach 时清掉。on_edit 复用同一 sink。
    sink_slot: Option<HighlightSink>,
    /// 上一次解析出的 Tree。`None` 表示尚未首次解析或上一轮失败（下次
    /// `on_edit` 走全量重解析把这两个槽都填回去）。
    tree: Option<Tree>,
    /// 与 `tree` 对应的 Snapshot：增量路径计算 InputEdit **旧端** Point 时
    /// 需要它（新端 Point 用 on_edit 收到的新 snapshot）。Snapshot 内部是 Arc
    /// 共享 rope，常驻一份开销可忽略。
    last_snapshot: Option<Snapshot>,
    /// 当前 desktop 上报的 viewport hint。`Some(range)` 时所有 on_edit 出的
    /// spans 只覆盖 `range`，通过 `sink.replace_range` 投递；`None` 走全文路径
    /// （全文 `replace_all`）。set_viewport 改变值时会立刻触发一次重 query
    /// （不重解析），让滚动后新区域 1–2 帧内补齐。
    viewport_hint: Option<TextRange>,
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
            cursor: QueryCursor::new(),
            sink_slot: None,
            tree: None,
            last_snapshot: None,
            viewport_hint: None,
        }
    }

    /// 全量解析缓冲区当前 snapshot，把高亮区间推给 sink。
    ///
    /// 同时把新 Tree 与 Snapshot 落进 worker 缓存，供下一轮 `on_edit` 走增量。
    /// 缓存的 `tree` 不能跨 snapshot 复用而不调用 `Tree::edit`——这条规则由
    /// 增量路径的 `try_incremental` 保证。
    ///
    /// **viewport-aware**：parse 阶段必须走全文（tree-sitter 要构建整棵树），
    /// 但 query 阶段按 `viewport_hint` 分支：
    ///
    /// - `Some(range)`：先推一份 **空 `ReplaceAll`** 在 sink 上把 layer 锚到本
    ///   版本（清掉上一份高亮、初始化版本），再 `set_byte_range` 跑局部 query
    ///   并以 `ReplaceRange` 投递 viewport 段 spans。视口外保持空，等滚动 / 编辑
    ///   再增量补齐。冷启动 16 MiB rust attach 时这条路径让"高亮亮起"从全树
    ///   query 的 1.5 s 量级落到 viewport 段的 100–300 ms 量级。
    /// - `None`：保持原全文 query + `ReplaceAll` 路径。desktop 在 attach 时还
    ///   没确定 viewport（或显式清空 hint）时落到这里。
    fn run_full(&mut self, buffer: &BufferHandle, sink: &HighlightSink) {
        let snapshot = buffer.snapshot();
        let version = snapshot.version();
        let text =
            match snapshot.slice_byte_range(zom_engine::ByteOffset::ZERO, snapshot.len_bytes()) {
                Ok(text) => text.into_text().into_owned(),
                Err(_) => {
                    self.tree = None;
                    self.last_snapshot = None;
                    sink.replace_all(version, Vec::new());
                    return;
                }
            };
        let bytes = text.as_bytes();

        let Some(tree) = self.parser.parse(bytes, None) else {
            // parse 失败：推空 spans 清掉上一份高亮，与「parse 错误尽量产出已知 span，未识别区域留空」对齐（手册 §八 表）。
            self.tree = None;
            self.last_snapshot = None;
            sink.replace_all(version, Vec::new());
            return;
        };

        match self.viewport_hint {
            Some(range) => {
                // 先用空 ReplaceAll 把 layer 锚到本版本——既清掉上一轮残留，又给
                // 后续 ReplaceRange 一个可 in-place 替换的本版本起点。
                sink.replace_all(version, Vec::new());
                self.cursor
                    .set_byte_range(range.start().get()..range.end().get());
                let spans = collect_spans(&self.config, &mut self.cursor, &tree, bytes);
                reset_cursor_range(&mut self.cursor);
                sink.replace_range(version, range, spans);
            }
            None => {
                reset_cursor_range(&mut self.cursor);
                let spans = collect_spans(&self.config, &mut self.cursor, &tree, bytes);
                sink.replace_all(version, spans);
            }
        }
        self.tree = Some(tree);
        self.last_snapshot = Some(snapshot);
    }

    /// 尝试增量重解析并推送 spans；返回 `false` 表示需要调用方走全量解析。
    ///
    /// 失败路径：缓存缺失 / InputEdit 翻译失败 / `parse_with_options` 返回 `None`。
    /// 任一失败都把 `tree`/`last_snapshot` 槽清空让调用方走 `run_full` 兜底；
    /// 不在这里直接推 spans，避免「半增量半全量」的 sink 状态。
    fn try_incremental(
        &mut self,
        buffer: &BufferHandle,
        change: &ChangeSet,
        sink: &HighlightSink,
        new_version: BufferVersion,
    ) -> bool {
        let (Some(old_snapshot), Some(mut tree)) = (self.last_snapshot.take(), self.tree.take())
        else {
            return false;
        };
        // 增量路径要求缓存版本与本次事件版本 **恰好相邻**：上次解析的版本加一就是当前事件的 new_version。
        // 若中间有未被本 worker 看到的事件（例如刚跨过多事件 pump 的中间帧），tree.edit 链就缺失，无法保证增量结果正确。
        // 返还槽位让调用方走全量。
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

        // 有 viewport hint 时只 query viewport ± 缓冲区段，spans 走 `sink.replace_range`；layer 的远端 spans 保持不变。
        // 无 hint 时走全文 query + `replace_all`。
        // 首次 attach、桌面端尚未上报 viewport，或 viewport hint 被显式清空时都会落到这条路径。
        match self.viewport_hint {
            Some(range) => {
                self.cursor
                    .set_byte_range(range.start().get()..range.end().get());
                let provider = SnapshotTextProvider {
                    snapshot: &new_snapshot,
                };
                let spans = collect_spans(&self.config, &mut self.cursor, &new_tree, provider);
                reset_cursor_range(&mut self.cursor);
                sink.replace_range(version, range, spans);
            }
            None => {
                let bytes_owned = match new_snapshot
                    .slice_byte_range(ByteOffset::ZERO, new_snapshot.len_bytes())
                {
                    Ok(slice) => slice.into_text().into_owned(),
                    Err(_) => return false,
                };
                reset_cursor_range(&mut self.cursor);
                let spans = collect_spans(
                    &self.config,
                    &mut self.cursor,
                    &new_tree,
                    bytes_owned.as_bytes(),
                );
                sink.replace_all(version, spans);
            }
        }
        self.tree = Some(new_tree);
        self.last_snapshot = Some(new_snapshot);
        true
    }

    /// 立即就 `range` 跑一次 viewport-scoped query，把结果作为 `ReplaceRange`
    /// 推给 sink——不重 parse、不刷 tree/last_snapshot。
    ///
    /// 仅在 worker 已有 tree + last_snapshot + sink 时有效；调用方一般是
    /// `set_viewport` 在 hint 实际改变后触发，目的是让滚动后新可见区域 1–2
    /// 帧内就有 spans，而不必等到下一次按键。
    fn reissue_viewport_query(&mut self, range: TextRange) {
        let (Some(tree), Some(snapshot), Some(sink)) = (
            self.tree.as_ref(),
            self.last_snapshot.as_ref(),
            self.sink_slot.clone(),
        ) else {
            return;
        };
        let version = snapshot.version();
        self.cursor
            .set_byte_range(range.start().get()..range.end().get());
        let provider = SnapshotTextProvider { snapshot };
        let spans = collect_spans(&self.config, &mut self.cursor, tree, provider);
        reset_cursor_range(&mut self.cursor);
        sink.replace_range(version, range, spans);
    }
}

/// 把 `QueryCursor::set_byte_range` 回到全文范围——viewport-scoped query 完成后
/// 立刻 reset，保证后续 run_full 等全文路径不被上一次约束截断。
///
/// tree-sitter 内部把 byte range 端点 cast 成 u32；`u32::MAX as usize` 是
/// 它支持的最大值。
fn reset_cursor_range(cursor: &mut QueryCursor) {
    cursor.set_byte_range(0..(u32::MAX as usize));
}

/// 把 `ChangeSet`（旧坐标 edit 列表）翻译为 tree-sitter `InputEdit` 列表。
///
/// 每条 InputEdit 的旧端 Point 用 `old_snapshot.byte_to_point` 解码，新端 Point
/// 用 `new_snapshot.byte_to_point` 解码——这两侧坐标系不同源（一个是旧文本、
/// 一个是新文本），不能混用同一份 snapshot；详见改造方案 §4.4。
///
/// 任何一条 edit 解码失败（越界 / 非字符边界）就返回 `None`，由调用方全量
/// 重解析。Edit 列表已经过 `EditList::new` 排序且不重叠，按顺序逐条 `Tree::edit`
/// 即可保证 tree-sitter 看到的内部坐标连续推进。
fn translate_edits(
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

/// `tree_sitter::TextProvider` 实现：predicate 评估时按需借出 rope chunks，
/// 不复制节点字节。
///
/// 用于 `QueryCursor::captures` 的 predicate 路径（rust grammar 用了 `#match?`
/// 做大写常量 / 类型识别），让 worker 不必事先物化全文。tree-sitter 把 chunk
/// 流概念吸进 predicate 求值器内部，跨 chunk 拼接由它自己处理；调用方按任意
/// UTF-8 字节边界续读即可。
///
/// **零拷贝**：每个 `text(node)` 调用返回的迭代器只持 `&'snap Snapshot`
/// 引用，逐 chunk 借出 `&'snap [u8]`，不再为每个 predicate 节点 alloc 一个
/// `Vec<u8>`——16 MiB rust 一档单键 viewport-scoped query 内 predicate
/// 触发频次按"节点遍历数"线性增长，这里省下来的 alloc/copy 是大头。
struct SnapshotTextProvider<'snap> {
    snapshot: &'snap Snapshot,
}

/// 沿 rope 边界懒迭代的 chunk 序列；上界由节点 `end_byte` 收口。
struct SnapshotChunkIter<'snap> {
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
/// 语义沿用 tree-sitter 官方 highlighter 的事件模型，只去除我们当前不消费的
/// injections / locals 两条支路：
///
/// 1. **同 node 覆盖**：`captures` 按 source order 流出；同一 node 上多个 pattern
///    依次出现时，**后到的 pattern 胜出**——下游的 `(node, pattern_b)`
///    覆盖先到的 `(node, pattern_a)`。这条规则让 grammar 通过把更具体的 query
///    写在更下方实现优先级。
/// 2. **嵌套 inner-wins**：一个 capture 的 byte range 完全落在前一个未结束 capture
///    范围内时，新 capture 入栈作为当前生效高亮，旧 capture 在 inner 结束后**继续**
///    生效——产出 `[outer_prefix, inner, outer_suffix]` 三段 span。
/// 3. **栈空段不产 span**：未被任何 capture 覆盖的字节默认走主题前景色，
///    span 表不记录这种"无高亮"段落。
///
/// 实现用「延迟一拍处理 pending capture」模式：每收到新 capture 时
/// 把上一条 pending 与之比对——同 node 就替换 pending、不同 node 就先 finalize
/// 上一条再排上新的。`StreamingIterator` 不能 peek，这种延迟一拍写法是适配
/// 它的最简方式。
fn collect_spans<P, I>(
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

    fn attach(&mut self, buffer: BufferHandle, sink: HighlightSink) {
        self.sink_slot = Some(sink.clone());
        self.run_full(&buffer, &sink);
    }

    fn on_edit(&mut self, buffer: BufferHandle, change: &ChangeSet, version: BufferVersion) {
        let Some(sink) = self.sink_slot.clone() else {
            return;
        };
        // 多事件批量 pump 的中间事件：BufferHandle 已是该批最终状态，与本事件的 `new_version` 不一致。
        // 此时既不能增量（缺中间 snapshot），也不必全量（终态事件马上会再驱动一次），直接无操作。
        if buffer.version() != version {
            return;
        }
        if !self.try_incremental(&buffer, change, &sink, version) {
            self.run_full(&buffer, &sink);
        }
    }

    /// 中间编辑快路径：只走 `translate_edits + Tree::edit`，不重 parse、不 query、
    /// 不推 sink。
    ///
    /// 调用方（[`crate::syntax::worker`] 的 coalesce 路径）会把同 buffer 连续多个
    /// 编辑事件中除最后一条外都送到这里；最后一条走 [`Self::on_edit`] 做完整 reparse
    /// + viewport-scoped query + ReplaceRange。这样 N 次按键只产生**一次** reparse
    /// 与 sink push，避免中间产物被立刻覆盖的浪费。
    ///
    /// 三类回退情况都把缓存清空，让下一次 `on_edit` 走 `run_full`：
    /// 1. 缓存缺失（首次 attach / 上一轮回退过）。
    /// 2. 版本不连续——`last_snapshot.version() + 1 != new_version`（worker 漏看了
    ///    中间事件，tree.edit 链断裂）。
    /// 3. `translate_edits` 失败（旧 / 新 snapshot 坐标解码出错）。
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

    /// 中间编辑快路径：只走 `translate_edits + Tree::edit`，不重 parse、不 query、
    /// 不推 sink。
    ///
    /// 调用方（[`crate::syntax::worker`] 的 coalesce 路径）会把同 buffer 连续多个
    /// 编辑事件中除最后一条外都送到这里；最后一条走 [`Self::on_edit`] 做完整 reparse
    /// + viewport-scoped query + ReplaceRange。这样 N 次按键只产生**一次** reparse
    /// 与 sink push，避免中间产物被立刻覆盖的浪费。
    ///
    /// 三类回退情况都把缓存清空，让下一次 `on_edit` 走 `run_full`：
    /// 1. 缓存缺失（首次 attach / 上一轮回退过）。
    /// 2. 版本不连续——`last_snapshot.version() + 1 != new_version`（worker 漏看了
    ///    中间事件，tree.edit 链断裂）。
    /// 3. `translate_edits` 失败（旧 / 新 snapshot 坐标解码出错）。
    fn apply_pending_edit(
        &mut self,
        buffer: BufferHandle,
        change: &ChangeSet,
        version: BufferVersion,
    ) {
        // 与 on_edit 同样的中间事件守门：BufferHandle 已是该批最终状态时，
        // 没有"上一轮 snapshot"可对——直接清空缓存让下一次 on_edit 走 run_full。
        if buffer.version() != version {
            self.tree = None;
            self.last_snapshot = None;
            return;
        }
        let (Some(old_snapshot), Some(mut tree)) = (self.last_snapshot.take(), self.tree.take())
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
        // 不 parse、不 query。把「已 edit 但未 reparse」的 tree 与新 snapshot 留给
        // 下一次 apply_pending_edit / on_edit 继续推进。
        self.tree = Some(tree);
        self.last_snapshot = Some(new_snapshot);
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

/// 给一门语言的 provider 跑「装上 → 喂样本 → 至少产出一个 span」的烟雾测试。
///
/// 不断言具体 capture name——name 是 grammar 内部细节，对未来升级太脆；只要
/// 证明 provider 正确接进调度链路。需要更强断言的语言（如 Rust 这类语义稳定
/// 的语言）可以在自己的测试里额外检查。
#[cfg(test)]
pub(crate) fn smoke_test_provider<F>(language_id: LanguageId, sample: &str, make: F)
where
    F: FnOnce() -> HighlightWorker,
{
    use crate::BufferId;
    use crate::syntax::{BufferSyntaxState, SyntaxWorkerHandle, payload::syntax_layer_kind};
    use std::sync::Arc;
    use zom_engine::{Buffer, BufferConfig, MetadataLayers};

    let buffer = Buffer::from_text(sample.to_string(), BufferConfig::default()).unwrap();
    let mut layers = MetadataLayers::<HighlightSpan>::new();
    let provider: Box<dyn HighlightProvider> = Box::new(make());
    let worker = Arc::new(SyntaxWorkerHandle::spawn());
    let state = BufferSyntaxState::attach(
        BufferId::from_raw(1),
        language_id,
        provider,
        &buffer,
        &mut layers,
        worker.clone(),
        None,
    );
    // 异步：等 worker 把首份产物推到 sink，再 drain 到 layers。
    worker.wait_for_idle();
    state.drain_into_layers(buffer.version(), &mut layers);
    let layer = layers
        .layer(&syntax_layer_kind())
        .expect("syntax layer 必须存在");
    assert!(layer.len() > 0, "provider 应至少为样本文本产出一个 span");
}

#[cfg(test)]
mod tests {
    use super::*;

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

    use crate::syntax::SinkMessage;
    use std::sync::OnceLock;
    use zom_engine::{
        Buffer, BufferConfig, Edit as EngineEdit, TextRange as EngineTextRange, Transaction,
    };

    fn rust_config() -> Arc<SharedConfig> {
        static CACHE: OnceLock<Arc<SharedConfig>> = OnceLock::new();
        CACHE
            .get_or_init(|| {
                let language: Language = tree_sitter_rust::LANGUAGE.into();
                Arc::new(build_shared_config(language, tree_sitter_rust::HIGHLIGHTS_QUERY).unwrap())
            })
            .clone()
    }

    /// 取 worker 的 sink 当前 latest 产物（解析 ReplaceAll 消息）作为 span 列表。
    fn drain_latest(sink: &HighlightSink) -> Option<Vec<(TextRange, HighlightSpan)>> {
        let mut latest = None;
        for msg in sink.drain() {
            if let SinkMessage::ReplaceAll { spans, .. } = msg {
                latest = Some(spans);
            }
        }
        latest
    }

    fn apply_replace(buffer: &mut Buffer, start: usize, end: usize, replacement: &str) {
        let range = EngineTextRange::new(ByteOffset::new(start), ByteOffset::new(end)).unwrap();
        let edit = EngineEdit::replace(range, replacement.to_string());
        let tx = Transaction::from_edits(buffer.version(), vec![edit]).unwrap();
        buffer.apply_transaction(tx).unwrap();
    }

    /// 把 buffer 当前 pending DeltaEvent 依次喂给 worker 的 on_edit。
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

    /// 每次都从零 attach 一个 baseline worker 以等价于「全量重 parse」。
    fn baseline_spans(buffer: &Buffer) -> Vec<(usize, usize, String)> {
        let mut worker = HighlightWorker::new(LanguageId::new("rust"), rust_config());
        let sink = HighlightSink::new();
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
        let spans = drain_latest(&sink).expect("基线必须产出 ReplaceAll");
        spans
            .into_iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str().to_string()))
            .collect()
    }

    #[test]
    fn incremental_matches_full_after_edits() {
        // 几次小编辑（插入、替换、删除）后，增量 worker 的产物必须与每次都全量 parse 的 baseline 完全等价。
        let initial = "pub fn answer() -> i32 {\n    let value = 42;\n    value\n}\n".to_string();
        let mut buffer = Buffer::from_text(initial, BufferConfig::default()).unwrap();
        let mut worker = HighlightWorker::new(LanguageId::new("rust"), rust_config());
        let sink = HighlightSink::new();
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
        let _ = drain_latest(&sink);

        // 插入字符串里的注释、修改返回类型、删一行。
        let steps: &[(usize, usize, &str)] = &[
            // "i32" → "i64"
            (21, 22, "6"),
            // 在 `let value = 42;` 前插入 `// hi\n    `
            (29, 29, "// hi\n    "),
            // 删除尾随空行
            (66, 67, ""),
        ];
        for (start, end, replacement) in steps {
            apply_replace(&mut buffer, *start, *end, replacement);
            pump(&mut worker, &mut buffer);
            let actual = drain_latest(&sink).expect("增量编辑必须产出 ReplaceAll");
            let actual_tuples: Vec<(usize, usize, String)> = actual
                .iter()
                .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str().to_string()))
                .collect();
            let expected = baseline_spans(&buffer);
            assert_eq!(
                actual_tuples, expected,
                "增量 spans 必须等于编辑后的全量解析基线（{start}..{end} <- {replacement:?}）"
            );
        }

        // 增量缓存应当一直在线（既没因翻译失败也没因 parse 失败回退）。
        assert!(
            worker.tree.is_some(),
            "tree 缓存必须在连续增量编辑后保持有效"
        );
        assert!(worker.last_snapshot.is_some());
    }

    #[test]
    fn version_gap_falls_back_to_full() {
        // 模拟 worker 错过一个中间事件：缓存版本不等于 new_version - 1 时，
        // try_incremental 必须返回 false，由 on_edit 走 run_full 全量解析。
        let mut buffer =
            Buffer::from_text("fn a() {}\n".to_string(), BufferConfig::default()).unwrap();
        let mut worker = HighlightWorker::new(LanguageId::new("rust"), rust_config());
        let sink = HighlightSink::new();
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
        let _ = drain_latest(&sink);

        // 跑两次编辑，但只 drop 第一次的事件不喂给 worker——制造版本断层。
        apply_replace(&mut buffer, 9, 9, " "); // version → v1
        let _ = buffer.take_pending_events();
        apply_replace(&mut buffer, 10, 10, "// tail\n"); // version → v2
        let events = buffer.take_pending_events();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        worker.on_edit(
            BufferHandle::new(buffer.snapshot()),
            event.changeset(),
            event.new_version(),
        );

        let actual = drain_latest(&sink).expect("全量路径仍必须产出 ReplaceAll");
        let expected = baseline_spans(&buffer);
        let actual_tuples: Vec<(usize, usize, String)> = actual
            .iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str().to_string()))
            .collect();
        assert_eq!(
            actual_tuples, expected,
            "版本跳跃时全量路径必须产出完整解析 spans"
        );
        assert!(worker.tree.is_some(), "run_full 会重新填充缓存");
    }

    // ============== viewport-scoped ReplaceRange 护栏 ==============

    /// 把 sink 当前消息流按 FIFO 拆成 ReplaceAll 与 ReplaceRange 两类。
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
    fn viewport_hint_emits_replace_range_after_edit() {
        // 一份足够长的 rust：5 个函数，viewport 只覆盖前两个。
        // 编辑发生在第一个函数内时，on_edit 应当只产 viewport 范围内的 spans，而且产物以 ReplaceRange 形式投递。
        let source = "pub fn a() -> i32 { 1 }\npub fn b() -> i32 { 2 }\npub fn c() -> i32 { 3 }\npub fn d() -> i32 { 4 }\npub fn e() -> i32 { 5 }\n".to_string();
        // viewport = 前两行（前两个函数），其余 c/d/e 落在外面
        let viewport_end = source.find("pub fn c").unwrap();
        let viewport = TextRange::new(ByteOffset::new(0), ByteOffset::new(viewport_end)).unwrap();

        let mut buffer = Buffer::from_text(source, BufferConfig::default()).unwrap();
        let mut worker = HighlightWorker::new(LanguageId::new("rust"), rust_config());
        let sink = HighlightSink::new();
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
        // attach 起手是 ReplaceAll 铺底
        let (initial_all, initial_ranges) = split_messages(&sink);
        assert_eq!(initial_all.len(), 1, "attach 必须先推一份 ReplaceAll 铺底");
        assert!(initial_ranges.is_empty(), "attach 不应直接推 ReplaceRange");

        // 设 viewport hint：set_viewport 立即触发一次 viewport-scoped query。
        worker.set_viewport(Some(viewport));
        let (eager_all, eager_ranges) = split_messages(&sink);
        assert!(eager_all.is_empty(), "set_viewport 不应再推 ReplaceAll");
        assert_eq!(
            eager_ranges.len(),
            1,
            "set_viewport 必须立刻推一份 viewport 段 ReplaceRange，让滚动后新区域 1–2 帧内见高亮"
        );
        let (eager_range, eager_spans) = &eager_ranges[0];
        assert_eq!(*eager_range, viewport);
        assert!(
            eager_spans
                .iter()
                .all(|(r, _)| r.start().get() < viewport_end),
            "viewport 段产物的 span 起点必须全部落在 viewport 内"
        );

        // 在 viewport 内编辑：把 fn a 的 i32 改成 i64。
        let i32_pos = "pub fn a() -> ".len();
        apply_replace(&mut buffer, i32_pos + 2, i32_pos + 3, "6");
        pump(&mut worker, &mut buffer);

        let (after_all, after_ranges) = split_messages(&sink);
        assert!(
            after_all.is_empty(),
            "viewport hint 在线时 on_edit 不应再推 ReplaceAll"
        );
        assert_eq!(
            after_ranges.len(),
            1,
            "viewport hint 在线时 on_edit 必须以 ReplaceRange 投递"
        );
        let (after_range, after_spans) = &after_ranges[0];
        assert_eq!(*after_range, viewport);
        assert!(
            after_spans
                .iter()
                .all(|(r, _)| r.start().get() < viewport_end),
            "增量产物必须只含 viewport 内的 spans——超出 viewport 的 span 起点泄露表示 set_byte_range 未生效"
        );

        // 清掉 viewport hint：下一次 on_edit 回退到 ReplaceAll 全文模式。
        worker.set_viewport(None);
        let (cleared_all, cleared_ranges) = split_messages(&sink);
        assert!(
            cleared_all.is_empty() && cleared_ranges.is_empty(),
            "clear viewport 本身不应触发产物——回退由下一次 on_edit 驱动"
        );
        apply_replace(&mut buffer, i32_pos + 2, i32_pos + 3, "2");
        pump(&mut worker, &mut buffer);
        let (full_all, full_ranges) = split_messages(&sink);
        assert_eq!(
            full_all.len(),
            1,
            "viewport 清空后 on_edit 必须以 ReplaceAll 回退"
        );
        assert!(full_ranges.is_empty());
        let full_spans = &full_all[0];
        // 远处函数（fn e）也必须出现在全文模式产物里
        assert!(
            full_spans
                .iter()
                .any(|(r, _)| r.start().get() > viewport_end),
            "ReplaceAll 必须覆盖 viewport 之外的 spans"
        );
    }

    #[test]
    fn viewport_scoped_spans_equal_full_parse_filtered() {
        // 等价性：viewport-scoped 产物 ≡ 同范围内的全量产物子集。
        let source = "fn a() -> i32 { let x = 1; x }\nfn b() -> i32 { let y = 2; y }\nfn c() -> i32 { let z = 3; z }\n".to_string();
        let cutoff = source.find("fn c").unwrap();
        let viewport = TextRange::new(ByteOffset::new(0), ByteOffset::new(cutoff)).unwrap();

        let mut buffer = Buffer::from_text(source, BufferConfig::default()).unwrap();
        let mut worker = HighlightWorker::new(LanguageId::new("rust"), rust_config());
        let sink = HighlightSink::new();
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
        let _ = sink.drain(); // 丢 attach 铺底

        worker.set_viewport(Some(viewport));
        let (_, ranges) = split_messages(&sink);
        assert_eq!(ranges.len(), 1);
        let mut viewport_tuples = tuples(&ranges[0].1);
        viewport_tuples.sort();

        // 编辑一次让 worker 走 incremental + viewport-scoped 路径。
        let pos = "fn a() -> ".len();
        apply_replace(&mut buffer, pos + 2, pos + 3, "6"); // i32 → i62
        pump(&mut worker, &mut buffer);
        let (_, ranges) = split_messages(&sink);
        let mut incremental_tuples = tuples(&ranges[0].1);
        incremental_tuples.sort();

        // 基线：全量 parse 再过滤到 viewport 内。
        let mut baseline: Vec<(usize, usize, String)> = baseline_spans(&buffer)
            .into_iter()
            .filter(|(start, _, _)| *start < cutoff)
            .collect();
        baseline.sort();

        assert_eq!(
            incremental_tuples, baseline,
            "viewport-scoped 增量产物必须等于全量 baseline 在同区间内的子集"
        );
    }

    // ============== apply_pending_edit (Edit coalescing) 护栏 ==============

    /// 折叠路径等价性：前 N-1 条走 apply_pending_edit，最后一条走 on_edit，
    /// 产出的最终 spans 必须等于"每条都走 on_edit"的顺序应用结果。
    #[test]
    fn apply_pending_edit_then_on_edit_matches_sequential() {
        let initial = "pub fn answer() -> i32 {\n    let value = 42;\n    value\n}\n".to_string();
        let steps: &[(usize, usize, &str)] = &[
            (21, 22, "6"), // i32 → i62
            (29, 29, "// hi\n    "),
            (66, 67, ""), // 删除尾随空行
        ];

        // 路径 A：折叠——前 N-1 走 apply_pending_edit，最后一条走 on_edit。
        let coalesced_spans = {
            let mut buffer = Buffer::from_text(initial.clone(), BufferConfig::default()).unwrap();
            let mut worker = HighlightWorker::new(LanguageId::new("rust"), rust_config());
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

        // 路径 B：每条都走 on_edit。
        let sequential_spans = {
            let mut buffer = Buffer::from_text(initial, BufferConfig::default()).unwrap();
            let mut worker = HighlightWorker::new(LanguageId::new("rust"), rust_config());
            let sink = HighlightSink::new();
            worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
            let _ = sink.drain();

            for (start, end, replacement) in steps {
                apply_replace(&mut buffer, *start, *end, replacement);
                pump(&mut worker, &mut buffer);
            }
            drain_latest(&sink).expect("顺序路径必须产出 ReplaceAll")
        };

        let coalesced_tuples: Vec<(usize, usize, String)> = coalesced_spans
            .iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str().to_string()))
            .collect();
        let sequential_tuples: Vec<(usize, usize, String)> = sequential_spans
            .iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str().to_string()))
            .collect();
        assert_eq!(
            coalesced_tuples, sequential_tuples,
            "折叠路径最终 spans 必须等于每步都 on_edit 的顺序路径"
        );
    }

    /// 折叠路径下，前 N-1 条 apply_pending_edit 不能向 sink 投任何消息——
    /// 这是"省一次 ReplaceAll/ReplaceRange"收益的关键不变量。
    #[test]
    fn apply_pending_edit_does_not_push_sink() {
        let mut buffer =
            Buffer::from_text("fn a() {}\n".to_string(), BufferConfig::default()).unwrap();
        let mut worker = HighlightWorker::new(LanguageId::new("rust"), rust_config());
        let sink = HighlightSink::new();
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
        let _ = sink.drain(); // 丢 attach 铺底

        apply_replace(&mut buffer, 9, 9, " ");
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
    }

    // ============== viewport-aware attach 护栏 ==============

    /// 设 viewport_hint 后 attach：run_full 必须先推一份空 ReplaceAll 把 layer
    /// 锚到当前版本，再推 viewport 段 ReplaceRange——而**不是**全文 ReplaceAll。
    #[test]
    fn run_full_with_hint_emits_anchor_replace_all_plus_replace_range() {
        let source = "pub fn a() -> i32 { 1 }\npub fn b() -> i32 { 2 }\npub fn c() -> i32 { 3 }\npub fn d() -> i32 { 4 }\npub fn e() -> i32 { 5 }\n".to_string();
        let cutoff = source.find("pub fn c").unwrap();
        let viewport = TextRange::new(ByteOffset::new(0), ByteOffset::new(cutoff)).unwrap();

        let buffer = Buffer::from_text(source, BufferConfig::default()).unwrap();
        let mut worker = HighlightWorker::new(LanguageId::new("rust"), rust_config());
        let sink = HighlightSink::new();
        // 模拟 worker.process(Job::Attach { initial_viewport: Some(viewport), ... })
        // 的顺序：先 set_viewport（tree/sink 还没就位，reissue 自动 no-op），再 attach。
        worker.set_viewport(Some(viewport));
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());

        let (alls, ranges) = split_messages(&sink);
        assert_eq!(
            alls.len(),
            1,
            "viewport-aware attach 必须先推一条空 ReplaceAll 锚定版本，实际 {} 条",
            alls.len()
        );
        assert!(
            alls[0].is_empty(),
            "锚定 ReplaceAll 的 spans 必须为空，实际 {} 条",
            alls[0].len()
        );
        assert_eq!(
            ranges.len(),
            1,
            "viewport-aware attach 必须随后推一条 viewport 段 ReplaceRange，实际 {} 条",
            ranges.len()
        );
        let (got_range, got_spans) = &ranges[0];
        assert_eq!(*got_range, viewport, "ReplaceRange 范围必须等于 viewport");
        assert!(!got_spans.is_empty(), "viewport 内必须产出至少一个 span");
        assert!(
            got_spans.iter().all(|(r, _)| r.start().get() < cutoff),
            "ReplaceRange 内的 span 起点必须全部落在 viewport 内"
        );
    }

    /// viewport-aware attach 的 viewport 内 spans 必须等价于"全文 attach 后再过滤"。
    #[test]
    fn viewport_aware_attach_spans_equal_full_attach_filtered() {
        let source = "fn a() -> i32 { let x = 1; x }\nfn b() -> i32 { let y = 2; y }\nfn c() -> i32 { let z = 3; z }\n".to_string();
        let cutoff = source.find("fn c").unwrap();
        let viewport = TextRange::new(ByteOffset::new(0), ByteOffset::new(cutoff)).unwrap();

        // 路径 A：viewport-aware attach。
        let mut viewport_tuples = {
            let buffer = Buffer::from_text(source.clone(), BufferConfig::default()).unwrap();
            let mut worker = HighlightWorker::new(LanguageId::new("rust"), rust_config());
            let sink = HighlightSink::new();
            worker.set_viewport(Some(viewport));
            worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());
            let (_, ranges) = split_messages(&sink);
            assert_eq!(ranges.len(), 1);
            tuples(&ranges[0].1)
        };
        viewport_tuples.sort();

        // 基线：全文 attach 后过滤到 viewport 内。
        let buffer = Buffer::from_text(source, BufferConfig::default()).unwrap();
        let mut baseline: Vec<(usize, usize, String)> = baseline_spans(&buffer)
            .into_iter()
            .filter(|(start, _, _)| *start < cutoff)
            .collect();
        baseline.sort();

        assert_eq!(
            viewport_tuples, baseline,
            "viewport-aware attach 的产物必须等于全文 attach baseline 在同区间内的子集"
        );
    }

    /// hint=None 的 attach 仍然推全文 ReplaceAll——回归护栏，确保我们没有把无 hint
    /// 路径意外切到 viewport-only 模式。
    #[test]
    fn run_full_without_hint_still_emits_full_replace_all() {
        let buffer = Buffer::from_text(
            "pub fn main() { let x = 1; }".to_string(),
            BufferConfig::default(),
        )
        .unwrap();
        let mut worker = HighlightWorker::new(LanguageId::new("rust"), rust_config());
        let sink = HighlightSink::new();
        // 不调 set_viewport——保持 None。
        worker.attach(BufferHandle::new(buffer.snapshot()), sink.clone());

        let (alls, ranges) = split_messages(&sink);
        assert_eq!(alls.len(), 1, "无 hint attach 必须推 1 条 ReplaceAll");
        assert!(
            !alls[0].is_empty(),
            "无 hint attach 的 ReplaceAll 必须包含全文 spans"
        );
        assert!(ranges.is_empty(), "无 hint attach 不应推 ReplaceRange");
    }
}
