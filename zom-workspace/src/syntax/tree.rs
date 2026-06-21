//! `BufferSyntaxTree`：单缓冲区的"当前 tree-sitter 解析树（们）+ 对应 snapshot"共享态。
//!
//! ## 角色
//!
//! 把"语言配置 + 当前 Tree（多棵）+ 对应 Snapshot + 版本号"打包成一个**不可变快照**值，
//! paint 端通过 [`crate::syntax::SyntaxHighlights`] 统一查询——tree-sitter spans 与 LSP semantic tokens 在 [`crate::syntax::SyntaxHighlights::query_viewport`] 内部 overlay。
//!
//! ## 多层（layered）
//!
//! 一个缓冲区可以同时持有多棵 tree。当前唯一用到多层的语言是 markdown（block grammar + inline grammar，手册 §十四「markdown 例外」）。
//! 其余语言（rust / python / ...）走单层退化路径：`layers.len() == 1`，paint 端代价等同重构前。
//!
//! 多层契约：
//!
//! - **同一 snapshot + version**：所有层共享一份 `Snapshot`，对同一份 bytes 解析得到。
//! - **precedence 自下而上**：`layers[0]` 是底层（如 markdown block），`layers[i]`（i 越大）覆盖越靠上；
//! [`BufferSyntaxTree::query_viewport`] 用 [`overlay_layers`] 把多层 spans 合并成最终非重叠序列，上层在覆盖区间内胜出。
//! - **同步推进**：[`crate::syntax::SyntaxHighlightsSlot::try_edit`] 把同一批 InputEdit 应用到每层 tree。

use std::sync::Arc;

use tree_sitter::{QueryCursor, Tree};
use zom_engine::{BufferVersion, ByteOffset, Snapshot, TextRange};

use super::payload::HighlightSpan;
use super::providers::common::{
    SharedConfig, SnapshotTextProvider, collect_spans, reset_cursor_range,
};

/// 缓冲区当前语法树的**一层**：一份语言配置（query + capture lookup）+ 对应 tree。
///
/// 多层情境下（markdown）每个 `SyntaxLayer` 对应一种 grammar（block / inline）。
/// 单层情境下（rust / python / ...）全程只有 `layers[0]`。
#[derive(Clone)]
pub struct SyntaxLayer {
    /// 语言级共享配置（query + capture lookup）。每条语言一份，跨 buffer 复用。
    pub(crate) config: Arc<SharedConfig>,
    /// 该层当前的 tree——可能是 reparse 出来的"结构正确的 tree"，也可能是主线程 `tree.edit` 推进过坐标但还没 reparse 的 interpolate tree。
    pub(crate) tree: Tree,
}

impl std::fmt::Debug for SyntaxLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyntaxLayer")
            .field("root_end_byte", &self.tree.root_node().end_byte())
            .finish_non_exhaustive()
    }
}

/// 缓冲区当前的语法树快照——`layers` + `snapshot` + `version` 一致。
///
/// 不可变值类型：每次更新都用 `Arc<BufferSyntaxTree>` 整体替换。
/// `Tree::clone` 是 `O(1)`（tree-sitter 内部 `Arc` 共享），所以"克隆一份再 `tree.edit`"的代价只在 `tree.edit` 自身的 `O(log N)` 字节坐标推进上。
pub struct BufferSyntaxTree {
    /// 自下而上的解析层。`layers[0]` 是 base，索引越大优先级越高（覆盖区间内胜出）。
    /// 单层语言只放一个元素；markdown 放 block + inline 两个元素。
    pub(crate) layers: Vec<SyntaxLayer>,
    /// 所有层共享的 buffer snapshot。`SnapshotTextProvider` 走它读节点文本。
    pub(crate) snapshot: Snapshot,
    /// `snapshot.version()` 缓存——store 时比版本时少一次 snapshot 解引用。
    pub(crate) version: BufferVersion,
}

impl BufferSyntaxTree {
    /// 单层构造器——给 rust / python / ... 等不需要多 grammar 的语言用。
    pub(crate) fn single(
        config: Arc<SharedConfig>,
        tree: Tree,
        snapshot: Snapshot,
        version: BufferVersion,
    ) -> Self {
        Self {
            layers: vec![SyntaxLayer { config, tree }],
            snapshot,
            version,
        }
    }

    /// 多层构造器——给 markdown 等需要 block + inline 的语言用。
    ///
    /// `layers` 顺序即 precedence 顺序：`layers[0]` 最底，最后一个最顶。
    /// 调用方保证 `!layers.is_empty()`——空 `layers` 在调度层不合法。
    pub(crate) fn layered(
        layers: Vec<SyntaxLayer>,
        snapshot: Snapshot,
        version: BufferVersion,
    ) -> Self {
        debug_assert!(
            !layers.is_empty(),
            "BufferSyntaxTree::layered 不接受空 layers"
        );
        Self {
            layers,
            snapshot,
            version,
        }
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    /// **底层** tree——多层情境下指 `layers[0]`。
    ///
    /// 大多数 caller（测试 / bench）只关心主结构（block tree / 单 grammar tree），不需要遍历所有层。
    /// 多层查询由 [`Self::query_viewport`] 内部按 precedence 遍历全部层。
    pub fn tree(&self) -> &Tree {
        &self.layers[0].tree
    }

    /// 在 `viewport` 字节区间上跑所有层的 tree-sitter Query，按 precedence 合并，返回非重叠 `(range, span)` 列表。
    ///
    /// paint 阶段的"render-time query"入口：不缓存 spans、每帧重算，颜色 = 当前 trees 在 viewport 上的纯函数。
    ///
    /// **复用 `cursor`**：tree-sitter 的 [`QueryCursor`] 构造一次还好，每帧多次会累积分配；
    /// 调用方应当用 [`SyntaxQueryCursor`] 在 thread-local / surface 上缓存一份，每帧 paint 时借出。多层时同一 cursor 顺序复用，每层跑完会先 reset 再跑下一层。
    ///
    /// query 命中的字节范围一律落在 `viewport` 内（`set_byte_range`），返回后 cursor 被复位到全文范围，可立即用于下一次 query 而不会被上一次约束截断。
    pub fn query_viewport(
        &self,
        viewport: TextRange,
        cursor: &mut SyntaxQueryCursor,
    ) -> Vec<(TextRange, HighlightSpan)> {
        // 单层快路径：直接跑 collect_spans，省掉一次 overlay 调用与 Vec 包装。
        // rust / python 等单 grammar 语言走这条；维持与重构前一致的代价。
        if self.layers.len() == 1 {
            return self.collect_layer_spans(&self.layers[0], viewport, cursor);
        }

        // 多层：每层独立 query，按 precedence 自下而上叠加，上层胜出。
        let layered_spans: Vec<Vec<(TextRange, HighlightSpan)>> = self
            .layers
            .iter()
            .map(|layer| self.collect_layer_spans(layer, viewport, cursor))
            .collect();
        overlay_layers(layered_spans)
    }

    fn collect_layer_spans(
        &self,
        layer: &SyntaxLayer,
        viewport: TextRange,
        cursor: &mut SyntaxQueryCursor,
    ) -> Vec<(TextRange, HighlightSpan)> {
        let inner = &mut cursor.inner;
        inner.set_byte_range(viewport.start().get()..viewport.end().get());
        let provider = SnapshotTextProvider {
            snapshot: &self.snapshot,
        };
        let spans = collect_spans(&layer.config, inner, &layer.tree, provider);
        reset_cursor_range(inner);
        spans
    }
}

/// 跨帧复用的 tree-sitter [`QueryCursor`] 包装。
///
/// 渲染端在 surface / thread-local 上持一份，每帧借给 [`BufferSyntaxTree::query_viewport`]——`QueryCursor` 构造代价小但非零（tree-sitter 内部 alloc），跨帧复用能把这部分摊掉。
pub struct SyntaxQueryCursor {
    inner: QueryCursor,
}

impl SyntaxQueryCursor {
    pub fn new() -> Self {
        Self {
            inner: QueryCursor::new(),
        }
    }
}

impl Default for SyntaxQueryCursor {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SyntaxQueryCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyntaxQueryCursor").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for BufferSyntaxTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferSyntaxTree")
            .field("version", &self.version)
            .field("layers", &self.layers.len())
            .field("snapshot_len", &self.snapshot.len_bytes().get())
            .finish_non_exhaustive()
    }
}

// BufferSyntaxTreeSlot 已迁移至 super::highlights::SyntaxHighlightsSlot。
// tree.rs 仅保留树数据结构和查询逻辑，不再维护共享槽。

// =============================================================================
// 多层 span 合并（overlay）
// =============================================================================

/// 把多层有序非重叠 span 列表叠加为一份有序非重叠 span 列表。
///
/// `layers[0]` 是 base（最底，优先级最低）；索引越大越靠上，在覆盖区间内胜出。
/// 等价于按顺序两两 `overlay(low, high)`：高层在自己的字节范围内完全胜出，低层在高层未覆盖的字节段显示。
///
/// 复杂度 = O(Σ|layer|)，对每一对 (low, high) 走一次双指针扫描。
/// viewport-scoped query 下每层只有几十到几百个 span，每帧 paint 不构成瓶颈。
pub(crate) fn overlay_layers(
    layers: Vec<Vec<(TextRange, HighlightSpan)>>,
) -> Vec<(TextRange, HighlightSpan)> {
    let mut iter = layers.into_iter();
    let Some(mut acc) = iter.next() else {
        return Vec::new();
    };
    for upper in iter {
        acc = overlay_two(acc, upper);
    }
    acc
}

/// `top` 在自己的字节范围内完全胜出；`base` 在 `top` 未覆盖的字节段保留。
///
/// 输入：两份按 start byte 严格递增、非重叠的 span 列表。
/// 输出：合并后按 start byte 严格递增、非重叠的 span 列表。
fn overlay_two(
    mut base: Vec<(TextRange, HighlightSpan)>,
    top: Vec<(TextRange, HighlightSpan)>,
) -> Vec<(TextRange, HighlightSpan)> {
    if top.is_empty() {
        return base;
    }
    if base.is_empty() {
        return top;
    }

    let mut out: Vec<(TextRange, HighlightSpan)> = Vec::with_capacity(base.len() + top.len() * 2);
    let mut bi = 0usize;
    let mut ti = 0usize;

    while bi < base.len() || ti < top.len() {
        let b_start = base
            .get(bi)
            .map(|(r, _)| r.start().get())
            .unwrap_or(usize::MAX);
        let t_start = top
            .get(ti)
            .map(|(r, _)| r.start().get())
            .unwrap_or(usize::MAX);

        if t_start <= b_start {
            // 上层胜出：把 top[ti] 推入输出，并把被它覆盖的 base 区间裁掉。
            let top_span = top[ti];
            let t_end = top_span.0.end().get();
            out.push(top_span);
            ti += 1;

            // 完全落在 [t_start, t_end] 之内的 base span 整条丢弃。
            while bi < base.len() && base[bi].0.end().get() <= t_end {
                bi += 1;
            }
            // 跨过 t_end 的 base span 起点裁到 t_end，等待下一轮循环输出尾段。
            if bi < base.len() && base[bi].0.start().get() < t_end {
                let (br, bs) = base[bi];
                if let Ok(truncated) = TextRange::new(ByteOffset::new(t_end), br.end()) {
                    base[bi] = (truncated, bs);
                }
            }
        } else {
            // base 先开始：能否整段输出取决于它是否撞到下一个 top 起点。
            let (br, bs) = base[bi];
            let b_end = br.end().get();
            if t_start < b_end {
                // base 横跨 top 的起点：把 base 拦腰切成 [b_start, t_start) 输出，
                // 剩下的 [t_start, b_end) 留给下一轮（下一轮会进入上层胜出分支）。
                if t_start > br.start().get()
                    && let Ok(prefix) = TextRange::new(br.start(), ByteOffset::new(t_start))
                {
                    out.push((prefix, bs));
                }
                if t_start < b_end {
                    if let Ok(remainder) = TextRange::new(ByteOffset::new(t_start), br.end()) {
                        base[bi] = (remainder, bs);
                    }
                } else {
                    bi += 1;
                }
            } else {
                out.push((br, bs));
                bi += 1;
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::payload::{HighlightName, TokenModifiers};
    fn span(start: usize, end: usize, name: &'static str) -> (TextRange, HighlightSpan) {
        let range = TextRange::new(ByteOffset::new(start), ByteOffset::new(end)).unwrap();
        let span = HighlightSpan::new(HighlightName::new(name), TokenModifiers::EMPTY);
        (range, span)
    }

    fn names(v: &[(TextRange, HighlightSpan)]) -> Vec<(usize, usize, &'static str)> {
        v.iter()
            .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str()))
            .collect()
    }

    // Slot 测试已迁移至 super::highlights 模块。

    // ============== overlay 单测 ==============

    #[test]
    fn overlay_empty_layers_returns_empty() {
        assert!(overlay_layers(Vec::new()).is_empty());
        assert!(overlay_layers(vec![Vec::new()]).is_empty());
        assert!(overlay_layers(vec![Vec::new(), Vec::new()]).is_empty());
    }

    #[test]
    fn overlay_single_layer_is_passthrough() {
        let layer = vec![span(0, 10, "a"), span(20, 30, "b")];
        let merged = overlay_layers(vec![layer.clone()]);
        assert_eq!(names(&merged), names(&layer));
    }

    #[test]
    fn overlay_top_inside_base_splits_into_three_segments() {
        // base 覆盖 [0, 100]，top 覆盖 [40, 50]——markdown 里典型的「heading 内 emphasis」形状。
        // 期望输出：[0, 40] base, [40, 50] top, [50, 100] base。
        let base = vec![span(0, 100, "markup.heading")];
        let top = vec![span(40, 50, "markup.italic")];
        let merged = overlay_layers(vec![base, top]);
        assert_eq!(
            names(&merged),
            vec![
                (0, 40, "markup.heading"),
                (40, 50, "markup.italic"),
                (50, 100, "markup.heading"),
            ]
        );
    }

    #[test]
    fn overlay_multiple_tops_inside_one_base() {
        let base = vec![span(0, 100, "markup.heading")];
        let top = vec![span(10, 20, "markup.italic"), span(30, 40, "markup.strong")];
        let merged = overlay_layers(vec![base, top]);
        assert_eq!(
            names(&merged),
            vec![
                (0, 10, "markup.heading"),
                (10, 20, "markup.italic"),
                (20, 30, "markup.heading"),
                (30, 40, "markup.strong"),
                (40, 100, "markup.heading"),
            ]
        );
    }

    #[test]
    fn overlay_top_aligned_with_base_replaces_completely() {
        let base = vec![span(10, 20, "markup.heading")];
        let top = vec![span(10, 20, "markup.italic")];
        let merged = overlay_layers(vec![base, top]);
        assert_eq!(names(&merged), vec![(10, 20, "markup.italic")]);
    }

    #[test]
    fn overlay_top_outside_base_appended() {
        // top 落在 base 外的区间——base 在那段没覆盖，top 原样输出。
        let base = vec![span(0, 10, "a")];
        let top = vec![span(20, 30, "b")];
        let merged = overlay_layers(vec![base, top]);
        assert_eq!(names(&merged), vec![(0, 10, "a"), (20, 30, "b")]);
    }

    #[test]
    fn overlay_top_extends_past_base_end() {
        // top [40, 60] 跨过 base [0, 50] 的右端：base 在 [0, 40] 保留，top 完整输出 [40, 60]。
        let base = vec![span(0, 50, "a")];
        let top = vec![span(40, 60, "b")];
        let merged = overlay_layers(vec![base, top]);
        assert_eq!(names(&merged), vec![(0, 40, "a"), (40, 60, "b")]);
    }

    #[test]
    fn overlay_three_layers_higher_index_wins() {
        // 三层叠加，索引越大越靠上。
        let l0 = vec![span(0, 100, "base")];
        let l1 = vec![span(10, 50, "mid")];
        let l2 = vec![span(20, 30, "top")];
        let merged = overlay_layers(vec![l0, l1, l2]);
        assert_eq!(
            names(&merged),
            vec![
                (0, 10, "base"),
                (10, 20, "mid"),
                (20, 30, "top"),
                (30, 50, "mid"),
                (50, 100, "base"),
            ]
        );
    }
}
