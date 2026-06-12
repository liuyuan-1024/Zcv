//! Markdown provider —— 自实现 [`HighlightProvider`]，驱动 block + inline + fenced 注入三套 grammar。
//!
//! ## 为什么不复用 [`HighlightWorker`]
//!
//! 手册 §十四 把 markdown 定为「跨语言嵌入」的唯一例外：
//!
//! - **block ↔ inline 主从**：tree-sitter-md 的 inline grammar 不能独立解析整段源码，必须先用 block grammar 切出 `inline` / `pipe_table_cell` 节点的 included_ranges，再让 inline parser 只在这些区间里 parse。
//! - **fenced code 注入**：` ```rust ... ``` ` 等代码块的内容按 fence info-string 指定的语言再 parse 一次。语言查表在 [`super::injection::resolve_injection_language`]。
//!
//! 两条主从依赖都与通用 worker「单 grammar、parse 全文」的简单模型冲突，强塞进去会让通用路径背上一个只服务于一门语言的分支。
//!
//! 所以这里直接实现 [`HighlightProvider`]，把多 grammar 编排封闭在本文件内：通用 worker 维持单 tree 简单模型，markdown 的复杂度只此一例。
//!
//! [`HighlightWorker`]: crate::syntax::providers::common::HighlightWorker
//!
//! ## 算法
//!
//! 复刻 tree-sitter-md 上游 `MarkdownParser::parse_with`，再补一层 fence 注入：
//!
//! 1. block parser 全文 parse → `block_tree`。
//! 2. 走 `block_tree`，对每个 `kind == "inline"` 或 `"pipe_table_cell"` 的节点：
//!    - 把它的 byte range 切除子节点（named children）后剩下的「间隙」拼成 `Vec<Range>`，
//!    - `inline_parser.set_included_ranges(&ranges)`，
//!    - `inline_parser.parse(...)` 得到一棵 inline tree。
//! 3. 走 `block_tree`，对每个 `fenced_code_block` 节点：
//!    - 从 `info_string → language` 子节点切出语言名，查表拿 `Arc<SharedConfig>`；
//!    - `code_fence_content` 子节点的 byte range 作 `included_ranges`，用对应语言的临时 `Parser` 全量 parse。
//!    - 语言未识别 / 内容空 / parse 失败都跳过该 fence，不影响其他 fence。
//! 4. block / inline / fence 三类树按出现顺序保存。
//!
//! 增量路径：tree.edit 推所有树 → block 走 `parse(bytes, Some(&old_block))` → inline / fence 整体重 parse（included_ranges 一变就不能复用增量）。
//!
//! 上游算法没法直接拉来用：tree-sitter-md 的 `parser` feature 依赖 `tree-sitter 0.23`，workspace 锁在 `0.26.9`，
//! 启用 feature 会把两个 tree-sitter 版本同时拉进来。所以这里把算法搬过来重写。

use std::sync::{Arc, OnceLock};

use tree_sitter::{Node, Parser, Range, Tree};
use zom_engine::{BufferVersion, ByteOffset, ChangeSet, Snapshot};

use crate::syntax::LanguageId;
use crate::syntax::provider::{BufferHandle, HighlightProvider};
use crate::syntax::providers::common::{
    SharedConfig, build_shared_config, build_shared_config_with_normalize, translate_edits,
};
use crate::syntax::providers::injection::resolve_injection_language;
use crate::syntax::tree::{BufferSyntaxTree, BufferSyntaxTreeSlot, SyntaxLayer};

const MARKDOWN_BLOCK_QUERY_EXTENSION: &str = r#"
; zom 本地扩展：补齐 tree-sitter-md 随包 nvim query 未覆盖的 Markdown 源码标记。
[
  (task_list_marker_checked)
  (task_list_marker_unchecked)
] @markup.list

(language) @attribute

(pipe_table_header
  (pipe_table_cell) @markup.heading)

[
  (pipe_table_delimiter_cell)
  (pipe_table_align_left)
  (pipe_table_align_right)
] @punctuation.delimiter
"#;

fn extended_markdown_block_query() -> &'static str {
    static CELL: OnceLock<&'static str> = OnceLock::new();
    CELL.get_or_init(|| {
        Box::leak(
            format!(
                "{}\n{}",
                tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
                MARKDOWN_BLOCK_QUERY_EXTENSION
            )
            .into_boxed_str(),
        )
    })
}

fn block_config() -> Arc<SharedConfig> {
    static CELL: OnceLock<Arc<SharedConfig>> = OnceLock::new();
    CELL.get_or_init(|| {
        Arc::new(
            build_shared_config(
                tree_sitter_md::LANGUAGE.into(),
                extended_markdown_block_query(),
            )
            .expect("tree-sitter-md block 高亮配置必须构建"),
        )
    })
    .clone()
}

fn inline_config() -> Arc<SharedConfig> {
    static CELL: OnceLock<Arc<SharedConfig>> = OnceLock::new();
    CELL.get_or_init(|| {
        Arc::new(
            build_shared_config_with_normalize(
                tree_sitter_md::INLINE_LANGUAGE.into(),
                tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
                normalize_inline_capture,
            )
            .expect("tree-sitter-md inline 高亮配置必须构建"),
        )
    })
    .clone()
}

/// 把 inline grammar 的 capture 方言归一到主题使用的 canonical name。
///
/// inline grammar 内的 `text.literal` 指 `code_span` / `link_title`，要落到 `markup.raw.inline`，
/// 与 block grammar 里同名 capture（fenced/indented code block）归一到 `markup.raw.block` 区分。两条 normalize 不能合并。
///
/// `text.emphasis` / `text.strong` 是 nvim-treesitter 命名，主题命名空间用 `markup.italic` / `markup.bold`；其余命中走主表（uri / reference），不在表里的 capture 原样透传。
fn normalize_inline_capture(name: &str) -> &str {
    match name {
        "text.literal" => "markup.raw.inline",
        "text.emphasis" => "markup.italic",
        "text.strong" => "markup.bold",
        "text.uri" => "markup.link.url",
        "text.reference" => "markup.link.text",
        other => other,
    }
}

/// 工厂——注册到 [`crate::syntax::LanguageRegistry`]。
pub fn new_provider() -> MarkdownProvider {
    MarkdownProvider::new()
}

/// markdown 专属 provider：自管 block tree + inline trees + snapshot。
pub struct MarkdownProvider {
    block_config: Arc<SharedConfig>,
    inline_config: Arc<SharedConfig>,
    block_parser: Parser,
    inline_parser: Parser,
    state: Option<ParseState>,
}

/// 一次成功解析的完整产物——block + inline + fence + 对应 snapshot。任一字段缺位都不该出现：
/// 解析失败统一退化为 `state = None`，由下一轮 `attach` / `run_full` 重新填回。
struct ParseState {
    block_tree: Tree,
    /// 按文档字节序排列。每棵对应 block tree 里一个 `inline` 或 `pipe_table_cell` 节点。
    inline_trees: Vec<Tree>,
    /// 按文档字节序排列。每棵对应 block tree 里一个识别成功的 `fenced_code_block`——
    /// 未识别语言 / 空内容 / parse 失败的 fence 在这里不出现（让 block grammar 的 `markup.raw.block` 兜底）。
    fence_trees: Vec<FenceTree>,
    snapshot: Snapshot,
}

/// 一条 fenced code 注入树：携带它该用哪门语言的 `SharedConfig` 跑 query。
/// `tree` 用 [`tree_sitter::Parser::set_included_ranges`] 限定在 `code_fence_content` 范围内 parse 而来，
/// 节点坐标仍是文档全局的（tree-sitter included_ranges 的语义就是这样），paint 端可以直接按 viewport 切。
struct FenceTree {
    config: Arc<SharedConfig>,
    tree: Tree,
}

impl std::fmt::Debug for MarkdownProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MarkdownProvider")
            .field(
                "inline_trees",
                &self.state.as_ref().map(|s| s.inline_trees.len()),
            )
            .finish_non_exhaustive()
    }
}

impl MarkdownProvider {
    pub fn new() -> Self {
        let block_config = block_config();
        let inline_config = inline_config();
        let mut block_parser = Parser::new();
        block_parser
            .set_language(&block_config.language)
            .expect("tree-sitter-md block grammar ABI 必须匹配");
        let mut inline_parser = Parser::new();
        inline_parser
            .set_language(&inline_config.language)
            .expect("tree-sitter-md inline grammar ABI 必须匹配");
        Self {
            block_config,
            inline_config,
            block_parser,
            inline_parser,
            state: None,
        }
    }

    /// 全量解析：block 从零 parse → 走 block tree → 每个 inline 节点单独 parse inline tree → 每个 fenced_code_block 单独 parse 注入 tree。
    /// 任何环节失败统一清空 state，让下次 on_edit 再次尝试。
    fn run_full(&mut self, buffer: &BufferHandle) {
        let snapshot = buffer.snapshot();
        let Some(bytes) = read_full_bytes(&snapshot) else {
            self.state = None;
            return;
        };
        // block parser 先把 included_ranges 重置回全文。inline parser 共用 Parser 实例时也要 reset，
        // 否则上一轮 set_included_ranges 还残留——参考 tree-sitter-md 上游 parse_with 的开头清零。
        let _ = self.block_parser.set_included_ranges(&[]);
        let Some(block_tree) = self.block_parser.parse(&bytes, None) else {
            self.state = None;
            return;
        };
        let inline_trees =
            match parse_inline_trees(&mut self.inline_parser, &block_tree, &bytes, None) {
                Some(trees) => trees,
                None => {
                    self.state = None;
                    return;
                }
            };
        // fence 注入失败仅丢弃失败的那条 fence，不会让整个 attach 退化——与 inline 不同：
        // inline 是 markdown 自身能力，全垮就该回退；
        // fence 是「外语言注入」，一条挂掉不该牵连整个 buffer 的 markdown 高亮。
        let fence_trees = parse_fence_trees(&block_tree, &bytes);
        self.state = Some(ParseState {
            block_tree,
            inline_trees,
            fence_trees,
            snapshot,
        });
    }

    /// 尝试增量：推进 block_tree 与所有 inline_tree 的坐标，
    /// block 用 `parse(_, Some(&old))` 走 tree-sitter 增量，
    /// inline 整体重 parse（included_ranges 一变就不能复用增量）。
    /// 任一步失败返回 false 让调用方走 `run_full`。
    fn try_incremental(
        &mut self,
        buffer: &BufferHandle,
        change: &ChangeSet,
        new_version: BufferVersion,
    ) -> bool {
        let Some(old_state) = self.state.take() else {
            return false;
        };
        if old_state.snapshot.version().get().saturating_add(1) != new_version.get() {
            return false;
        }
        let new_snapshot = buffer.snapshot();
        let Some(new_bytes) = read_full_bytes(&new_snapshot) else {
            return false;
        };

        let input_edits = match translate_edits(change, &old_state.snapshot, &new_snapshot) {
            Some(edits) => edits,
            None => return false,
        };

        // 推进 block tree。
        let mut block_tree = old_state.block_tree;
        for ie in &input_edits {
            block_tree.edit(ie);
        }
        // 推进所有 inline tree——后面虽然要整体重 parse，但坐标推进确保失败回退时 state 仍自洽，
        // 也让上游算法的「按位置喂 old_tree」能尽量命中。
        let mut inline_trees_advanced: Vec<Tree> = old_state
            .inline_trees
            .into_iter()
            .map(|mut t| {
                for ie in &input_edits {
                    t.edit(ie);
                }
                t
            })
            .collect();

        let _ = self.block_parser.set_included_ranges(&[]);
        let Some(new_block_tree) = self.block_parser.parse(&new_bytes, Some(&block_tree)) else {
            // block 增量都没成，回退给 run_full。state 已经被 take 走，保持 None 让外层 run_full 重建。
            return false;
        };

        // inline 无增量：用新的 block tree 重新算 included_ranges，整体重 parse。
        // 把 advanced 后的旧 inline trees 按位置喂给 parser；位置数量对得上时 tree-sitter 内部能复用部分子节点。
        let new_inline_trees = match parse_inline_trees(
            &mut self.inline_parser,
            &new_block_tree,
            &new_bytes,
            Some(&mut inline_trees_advanced),
        ) {
            Some(trees) => trees,
            None => return false,
        };

        // fence 注入：included_ranges 跟着 block 节点位置变，无法走 tree-sitter 增量；
        // 与 inline 一样整体重 parse。临时 parser 不缓存，每条 fence 一份。
        let new_fence_trees = parse_fence_trees(&new_block_tree, &new_bytes);

        self.state = Some(ParseState {
            block_tree: new_block_tree,
            inline_trees: new_inline_trees,
            fence_trees: new_fence_trees,
            snapshot: new_snapshot,
        });
        true
    }
}

impl Default for MarkdownProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl HighlightProvider for MarkdownProvider {
    fn language(&self) -> LanguageId {
        LanguageId::new("markdown")
    }

    fn attach(&mut self, buffer: BufferHandle) {
        self.run_full(&buffer);
    }

    fn on_edit(&mut self, buffer: BufferHandle, change: &ChangeSet, version: BufferVersion) {
        // 与通用 HighlightWorker 同步规则：批量 pump 的中间事件，BufferHandle 已是终态，
        // 不与本事件 new_version 一致，直接无操作（终态事件马上会再来一次）。
        if buffer.version() != version {
            return;
        }
        if !self.try_incremental(&buffer, change, version) {
            self.run_full(&buffer);
        }
    }

    fn detach(&mut self) {
        self.state = None;
    }

    /// 把 block + 所有 inline tree + 所有 fence tree 打包成多层 [`BufferSyntaxTree`] 写到共享 slot。
    ///
    /// 层序自下而上：block → inline → fence。
    ///
    /// - **block**（layers[0]）兜底，覆盖整篇文档的结构标记；
    /// - **inline**（中间层）按文档字节序排；字节范围两两不相交（不同 paragraph / heading），互不覆盖；覆盖 block 在对应区间的产出（heading 内的 emphasis、paragraph 内的 code_span 等）。
    /// - **fence**（最顶层）按文档字节序排；字节范围两两不相交（不同的 fenced_code_block），互不覆盖；覆盖 block 给 fence content 标的 `markup.raw.block`。
    ///
    /// inline 与 fence 之间天然不相交：fenced_code_block 是 block 级节点，不会被 `inline` 包裹。
    /// 所以三类层之间放成何种顺序对最终 spans 不影响，按上述顺序排只是为了让 export 与 overlay 的 precedence 含义自洽，便于排查。
    fn export_syntax_tree(&self, slot: &BufferSyntaxTreeSlot) {
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let mut layers: Vec<SyntaxLayer> =
            Vec::with_capacity(1 + state.inline_trees.len() + state.fence_trees.len());
        layers.push(SyntaxLayer {
            config: self.block_config.clone(),
            tree: state.block_tree.clone(),
        });
        for tree in &state.inline_trees {
            layers.push(SyntaxLayer {
                config: self.inline_config.clone(),
                tree: tree.clone(),
            });
        }
        for fence in &state.fence_trees {
            layers.push(SyntaxLayer {
                config: fence.config.clone(),
                tree: fence.tree.clone(),
            });
        }
        let version = state.snapshot.version();
        slot.store_if_newer(BufferSyntaxTree::layered(
            layers,
            state.snapshot.clone(),
            version,
        ));
    }
}

// =============================================================================
// 内部辅助
// =============================================================================

fn read_full_bytes(snapshot: &Snapshot) -> Option<Vec<u8>> {
    snapshot
        .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
        .ok()
        .map(|s| s.into_text().into_owned().into_bytes())
}

/// 遍历 `block_tree`，对每个 `inline` / `pipe_table_cell` 节点用 included_ranges 跑 inline parser。
///
/// `old_inline_trees`：若存在且与新 block tree 上 inline 节点的顺序一致，按位置喂给 `parser.parse(_, Some(&old))`
/// 让 tree-sitter 复用未变更的子树。`None` 表示从零 parse 全部 inline。
///
/// 算法直接复刻 tree-sitter-md 上游 `MarkdownParser::parse_with`：
/// 内层循环既找 inline / pipe_table_cell 节点，又跳过它们的子节点（named children）所占字节段——
/// 因为这些子节点已经在 block tree 里有正确归类（如 fenced code block 内的 `language`），不该让 inline grammar 重新解析。
///
/// 返回 `None` 表示某一棵 inline 解析失败（grammar ABI / parser 状态错乱），调用方走 run_full 全量回退。
fn parse_inline_trees(
    parser: &mut Parser,
    block_tree: &Tree,
    bytes: &[u8],
    mut old_inline_trees: Option<&mut Vec<Tree>>,
) -> Option<Vec<Tree>> {
    let mut inline_trees: Vec<Tree> = Vec::new();
    let mut tree_cursor = block_tree.walk();
    let mut i: usize = 0;

    'outer: loop {
        // 内层循环找下一个 inline / pipe_table_cell 节点。
        let node = loop {
            let kind = tree_cursor.node().kind();
            if kind == "inline" || kind == "pipe_table_cell" || !tree_cursor.goto_first_child() {
                while !tree_cursor.goto_next_sibling() {
                    if !tree_cursor.goto_parent() {
                        break 'outer;
                    }
                }
            }
            let kind = tree_cursor.node().kind();
            if kind == "inline" || kind == "pipe_table_cell" {
                break tree_cursor.node();
            }
        };

        // 收集 included_ranges：把 node 的 byte range 切掉「named children」后的间隙。
        let mut range = node.range();
        let mut ranges: Vec<Range> = Vec::new();
        if tree_cursor.goto_first_child() {
            while tree_cursor.goto_next_sibling() {
                if !tree_cursor.node().is_named() {
                    continue;
                }
                let child_range = tree_cursor.node().range();
                ranges.push(Range {
                    start_byte: range.start_byte,
                    start_point: range.start_point,
                    end_byte: child_range.start_byte,
                    end_point: child_range.start_point,
                });
                range.start_byte = child_range.end_byte;
                range.start_point = child_range.end_point;
            }
            tree_cursor.goto_parent();
        }
        ranges.push(range);

        // included_ranges 设错（如端点越界 / 顺序错乱）就 None 失败回退。
        parser.set_included_ranges(&ranges).ok()?;

        let old_tree = old_inline_trees.as_mut().and_then(|v| v.get(i));
        let inline_tree = parser.parse(bytes, old_tree)?;
        inline_trees.push(inline_tree);
        i += 1;
    }

    // 退出前把 inline parser 的 included_ranges 还原全文，避免下次跨 provider 复用时残留。
    let _ = parser.set_included_ranges(&[]);
    Some(inline_trees)
}

/// 遍历 `block_tree`，对每个 `fenced_code_block` 节点跑「fence 内注入」。
///
/// 单条 fence 的处理流程：
///
/// 1. 取 `info_string → language` 子节点，把它的字节段从 `bytes` 切出来当语言名（一般是 ASCII 单词）；
/// 2. `resolve_injection_language` 查表拿 `Arc<SharedConfig>`；未识别 → 跳过该 fence；
/// 3. 取 `code_fence_content` 子节点的 byte range；不存在或空内容 → 跳过；
/// 4. 临时 [`Parser`] + `set_language` + `set_included_ranges(&[content_range])` + `parse(bytes, None)`；
/// 5. parse 失败 → 跳过该 fence。
///
/// 任一 fence 失败都只丢自己那条注入，**不**让其余 fence / inline / block 受牵连——fence 注入是「外语言能力」，单条挂掉与「该语言不在 Tier 1」等价，都让 block grammar 的 `markup.raw.block` 兜底。
///
/// 不缓存 parser：每条 fence 一个新 [`Parser`]。`set_language` 在 fence 个数很多时会累，
/// 典型 markdown 文档（fence ≤ 几十个）单次 attach / on_edit 加 100µs 量级，不构成瓶颈；
/// 若 bench 显示拖帧再加 per-language `Parser` 缓存。
fn parse_fence_trees(block_tree: &Tree, bytes: &[u8]) -> Vec<FenceTree> {
    let mut out: Vec<FenceTree> = Vec::new();
    walk_fenced_code_blocks(block_tree.root_node(), &mut |fence_node| {
        if let Some(fence) = try_inject_fence(fence_node, bytes) {
            out.push(fence);
        }
    });
    out
}

/// 在 block_tree 上做先序遍历，对每个 `fenced_code_block` 节点调一次回调。
///
/// 不递归进 fenced_code_block 内部（fence 内部的子节点不可能再含 fenced_code_block）——剪枝一层。
fn walk_fenced_code_blocks<F: FnMut(Node<'_>)>(node: Node<'_>, callback: &mut F) {
    if node.kind() == "fenced_code_block" {
        callback(node);
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_fenced_code_blocks(child, callback);
    }
}

/// 解析单个 `fenced_code_block` 节点为一条 [`FenceTree`]。任一前置条件失败返回 `None`。
fn try_inject_fence(fence_node: Node<'_>, bytes: &[u8]) -> Option<FenceTree> {
    let (language_name, content_range) = extract_language_and_content(fence_node, bytes)?;

    // 空内容（` ```rust\n``` ` 这种）直接跳过——`set_included_ranges(&[empty_range])` 行为未定义，而且空 fence 本身就没什么可高亮的。
    if content_range.start_byte >= content_range.end_byte {
        return None;
    }

    let config = resolve_injection_language(language_name)?;

    let mut parser = Parser::new();
    parser.set_language(&config.language).ok()?;
    parser.set_included_ranges(&[content_range]).ok()?;
    let tree = parser.parse(bytes, None)?;
    Some(FenceTree { config, tree })
}

/// 从 `fenced_code_block` 节点取出 (语言名, content_range)。
///
/// `info_string` 是可选子节点；不存在或者它里面没有 `language` 子节点都视为「未识别」。
fn extract_language_and_content<'a>(
    fence_node: Node<'_>,
    bytes: &'a [u8],
) -> Option<(&'a str, Range)> {
    let mut language_name: Option<&str> = None;
    let mut content_range: Option<Range> = None;
    let mut cursor = fence_node.walk();
    for child in fence_node.children(&mut cursor) {
        match child.kind() {
            "info_string" => {
                // info_string 一般包含一个 `language` 子节点；
                // 偶尔还跟着 `code_fence_info_string` 之类的尾部信息，本注入路径只关心 language。
                let mut sub = child.walk();
                for inner in child.children(&mut sub) {
                    if inner.kind() == "language" {
                        let start = inner.start_byte();
                        let end = inner.end_byte();
                        if start <= end && end <= bytes.len() {
                            language_name = std::str::from_utf8(&bytes[start..end]).ok();
                        }
                    }
                }
            }
            "code_fence_content" => {
                content_range = Some(child.range());
            }
            _ => {}
        }
    }
    Some((language_name?, content_range?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BufferId;
    use crate::syntax::payload::HighlightSpan;
    use crate::syntax::tree::SyntaxQueryCursor;
    use crate::syntax::{BufferSyntax, SyntaxWorkerHandle};
    use std::sync::Arc;
    use zom_engine::{Buffer, BufferConfig, TextRange};

    fn attach_markdown(buffer: &Buffer) -> (BufferSyntax, Arc<SyntaxWorkerHandle>) {
        let worker = Arc::new(SyntaxWorkerHandle::spawn());
        let syntax = BufferSyntax::attach(
            BufferId::from_raw(1),
            LanguageId::new("markdown"),
            Box::new(MarkdownProvider::new()),
            buffer,
            worker.clone(),
        );
        worker.wait_for_idle_for_test_or_bench();
        (syntax, worker)
    }

    fn span_names(spans: &[(TextRange, HighlightSpan)]) -> Vec<(usize, usize, String)> {
        spans
            .iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str().to_string()))
            .collect()
    }

    fn query_full(syntax: &BufferSyntax, buffer: &Buffer) -> Vec<(TextRange, HighlightSpan)> {
        let tree = syntax
            .tree_slot()
            .load()
            .expect("attach 完成后 slot 必须有 tree");
        let viewport =
            TextRange::new(ByteOffset::ZERO, buffer.snapshot().len_bytes()).expect("全文区间");
        let mut cursor = SyntaxQueryCursor::new();
        tree.query_viewport(viewport, &mut cursor)
    }

    #[test]
    fn block_and_inline_layers_produce_spans() {
        // 最基本的两层联动：heading 由 block 标 markup.heading，内文 emphasis 由 inline 标 markup.italic。
        let buffer =
            Buffer::from_text("# Hello *world* end\n".to_string(), BufferConfig::default())
                .unwrap();
        let (syntax, _w) = attach_markdown(&buffer);
        let spans = query_full(&syntax, &buffer);
        let names = span_names(&spans);

        assert!(
            names.iter().any(|(_, _, n)| n == "markup.italic"),
            "inline 应当产出 markup.italic：实际 {names:?}"
        );
        assert!(
            names.iter().any(|(_, _, n)| n == "markup.heading"),
            "block 应当产出 markup.heading：实际 {names:?}"
        );
    }

    #[test]
    fn code_span_in_paragraph_yields_markup_raw_inline() {
        // paragraph 内 `code` 应被 inline grammar 标为 markup.raw.inline——区别于 block grammar 的 markup.raw.block。
        let buffer = Buffer::from_text(
            "before `inline code` after\n".to_string(),
            BufferConfig::default(),
        )
        .unwrap();
        let (syntax, _w) = attach_markdown(&buffer);
        let spans = query_full(&syntax, &buffer);
        let names = span_names(&spans);
        assert!(
            names.iter().any(|(_, _, n)| n == "markup.raw.inline"),
            "inline code span 应当被标 markup.raw.inline：实际 {names:?}"
        );
        assert!(
            !names.iter().any(|(_, _, n)| n == "markup.raw.block"),
            "paragraph 内的 code span 不应被误标为 markup.raw.block：实际 {names:?}"
        );
    }

    #[test]
    fn fenced_rust_block_renders_rust_tokens() {
        // ```rust ... ``` 内的代码必须按 rust grammar 出 rust spans，覆盖 block 的 markup.raw.block。
        // 同一份文本里既要有 rust 命名（keyword / function / ...），也要保留 fence delimiters 处的 markup.raw.block。
        let buffer = Buffer::from_text(
            "```rust\nfn main() { let s = \"hi\"; }\n```\n".to_string(),
            BufferConfig::default(),
        )
        .unwrap();
        let (syntax, _w) = attach_markdown(&buffer);
        let spans = query_full(&syntax, &buffer);
        let names = span_names(&spans);
        let name_set: std::collections::HashSet<&str> =
            names.iter().map(|(_, _, n)| n.as_str()).collect();
        assert!(
            name_set.contains("keyword"),
            "fence 注入应当让 rust grammar 标 keyword：实际 {names:?}"
        );
        assert!(
            name_set.contains("function") || name_set.contains("string"),
            "fence 注入应当让 rust grammar 至少出 function 或 string 之一：实际 {names:?}"
        );
    }

    #[test]
    fn unknown_fence_language_falls_back_to_raw_block() {
        // 未识别语言（不在 injection 表里）的 fence：fence_trees 为空，block grammar 的 markup.raw.block 兜底。
        let buffer = Buffer::from_text(
            "```nosuchlang\nlet x = 1;\n```\n".to_string(),
            BufferConfig::default(),
        )
        .unwrap();
        let (syntax, _w) = attach_markdown(&buffer);
        let spans = query_full(&syntax, &buffer);
        let names = span_names(&spans);
        let name_set: std::collections::HashSet<&str> =
            names.iter().map(|(_, _, n)| n.as_str()).collect();
        assert!(
            name_set.contains("markup.raw.block"),
            "未识别语言的 fence 必须落 markup.raw.block 兜底：实际 {names:?}"
        );
        assert!(
            !name_set.contains("keyword"),
            "未识别语言不该误注入任何 grammar：实际 {names:?}"
        );
    }

    #[test]
    fn fence_without_info_string_falls_back_to_raw_block() {
        // 无 info_string 的 fence（直接 ``` 起手）：language 缺失 → 跳过注入，block grammar 兜底。
        let buffer = Buffer::from_text(
            "```\nplain text\n```\n".to_string(),
            BufferConfig::default(),
        )
        .unwrap();
        let (syntax, _w) = attach_markdown(&buffer);
        let spans = query_full(&syntax, &buffer);
        let names = span_names(&spans);
        let name_set: std::collections::HashSet<&str> =
            names.iter().map(|(_, _, n)| n.as_str()).collect();
        assert!(
            name_set.contains("markup.raw.block"),
            "无 info_string 的 fence 仍走 markup.raw.block 兜底：实际 {names:?}"
        );
    }

    #[test]
    fn fence_injection_supports_language_aliases() {
        // 用 `rs` 别名应当与 `rust` 等价命中。
        let buffer = Buffer::from_text(
            "```rs\nfn main() {}\n```\n".to_string(),
            BufferConfig::default(),
        )
        .unwrap();
        let (syntax, _w) = attach_markdown(&buffer);
        let names = span_names(&query_full(&syntax, &buffer));
        let name_set: std::collections::HashSet<&str> =
            names.iter().map(|(_, _, n)| n.as_str()).collect();
        assert!(
            name_set.contains("keyword"),
            "别名 `rs` 应当走 rust grammar：实际 {names:?}"
        );
    }

    #[test]
    fn pipe_table_cell_inline_emphasis_wins_over_heading() {
        // | foo |  → block 把表头 cell 标 markup.heading；
        // | *bar* | → inline 在 bar 上标 markup.italic，应覆盖 heading 的字节段。
        let buffer = Buffer::from_text(
            "| foo |\n| --- |\n| *bar* |\n".to_string(),
            BufferConfig::default(),
        )
        .unwrap();
        let (syntax, _w) = attach_markdown(&buffer);
        let spans = query_full(&syntax, &buffer);
        let names = span_names(&spans);
        assert!(
            names.iter().any(|(_, _, n)| n == "markup.italic"),
            "pipe_table_cell 内 emphasis 应当被 inline grammar 标 markup.italic：实际 {names:?}"
        );
    }

    #[test]
    fn incremental_edit_inside_fence_keeps_spans_consistent_with_baseline() {
        // fence 注入版本的增量护栏：在 rust fence 内部插字符后，spans 必须与从零 attach 同样文本的 baseline 一致。
        // 这条覆盖了 fence_trees 在 try_incremental 路径上整体重 parse 的等价性。
        let initial = "Intro\n\n```rust\nfn main() {}\n```\n\nEnd.\n".to_string();
        let mut buffer = Buffer::from_text(initial, BufferConfig::default()).unwrap();
        let (syntax, worker) = attach_markdown(&buffer);

        // 在 `fn main() {}` 的 `{` 后插入一个 let 语句。
        // 文本 "Intro\n\n```rust\nfn main() {}\n```\n\nEnd.\n"
        //                            ^^^^^^^^^^^^^^ 这一段在 fence 内
        // 我们插在 `{` 与 `}` 之间。
        let insert_byte = buffer
            .snapshot()
            .slice_byte_range(ByteOffset::ZERO, buffer.snapshot().len_bytes())
            .unwrap()
            .into_text()
            .into_owned()
            .find("{}")
            .map(|p| p + 1)
            .expect("测试样本里必须含 {}");
        buffer
            .insert(ByteOffset::new(insert_byte), " let x = 1; ")
            .unwrap();
        let events = buffer.take_pending_events();
        let event = events.into_iter().next().unwrap();
        syntax.handle_edit(&buffer, &event);
        worker.wait_for_idle_for_test_or_bench();

        let incremental = span_names(&query_full(&syntax, &buffer));

        let snap = buffer.snapshot();
        let baseline_text = snap
            .slice_byte_range(ByteOffset::ZERO, snap.len_bytes())
            .unwrap()
            .into_text()
            .into_owned();
        let baseline_buffer = Buffer::from_text(baseline_text, BufferConfig::default()).unwrap();
        let (baseline_syntax, _bw) = attach_markdown(&baseline_buffer);
        let baseline = span_names(&query_full(&baseline_syntax, &baseline_buffer));

        assert_eq!(
            incremental, baseline,
            "fence 内编辑后增量 spans 必须与从零 parse 的 baseline 一致"
        );
    }

    #[test]
    fn incremental_edit_keeps_spans_consistent_with_baseline() {
        // 增量护栏：一次小编辑后，增量 spans 与从零 attach 同样文本的 baseline 完全一致。
        let initial = "# Hello *world*\n\nLine `code` two.\n".to_string();
        let mut buffer = Buffer::from_text(initial, BufferConfig::default()).unwrap();
        let (syntax, worker) = attach_markdown(&buffer);

        buffer.insert(ByteOffset::new(7), "X").unwrap();
        let events = buffer.take_pending_events();
        let event = events.into_iter().next().unwrap();
        syntax.handle_edit(&buffer, &event);
        worker.wait_for_idle_for_test_or_bench();

        let incremental = span_names(&query_full(&syntax, &buffer));

        // 用同样文本从零跑一遍。
        let snap = buffer.snapshot();
        let baseline_text = snap
            .slice_byte_range(ByteOffset::ZERO, snap.len_bytes())
            .unwrap()
            .into_text()
            .into_owned();
        let baseline_buffer = Buffer::from_text(baseline_text, BufferConfig::default()).unwrap();
        let (baseline_syntax, _bw) = attach_markdown(&baseline_buffer);
        let baseline = span_names(&query_full(&baseline_syntax, &baseline_buffer));

        assert_eq!(
            incremental, baseline,
            "markdown 增量后 spans 必须与从零 parse 的 baseline 一致"
        );
    }
}
