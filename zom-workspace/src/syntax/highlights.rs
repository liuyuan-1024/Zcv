//! `SyntaxHighlights`：统一的高亮查询入口——把 tree-sitter 与 LSP 语义 token 两套数据源内聚为一个 viewport query。
//!
//! ## 设计
//!
//! [`SyntaxHighlights`] 持有 tree-sitter 解析树 + 可选 LSP 语义 token spans；
//! [`SyntaxHighlights::query_viewport`] 内部先跑 tree-sitter base，再把 LSP spans 按 viewport 筛选后 overlay 上去（LSP 在重叠区间胜出），paint 端只面对一个类型、一次 query，不区分数据源。
//!
//! ## 并发模型
//!
//! [`SyntaxHighlightsSlot`] 用 `Arc<Mutex<…>>` 跨线程共享：
//!
//! - **worker 线程** 在 attach / reparse 后调 `store_tree` 写入最新树。
//! - **LspHost** 在收到 `semanticTokens/full` 后调 `store_lsp` 写入解码后的 spans。
//! - **主线程编辑入口** 在每次编辑时调 `try_edit` 推进 tree 字节坐标。
//! - **paint 端** 调 `load` 拿到 `Arc<SyntaxHighlights>` 后按 viewport 现查。

use std::sync::{Arc, Mutex};

use tree_sitter::InputEdit;
use zom_engine::{BufferVersion, Snapshot, TextRange};

use super::payload::HighlightSpan;
use super::tree::{BufferSyntaxTree, SyntaxLayer, SyntaxQueryCursor, overlay_layers};

/// 统一的高亮快照——tree-sitter 树 + 可选 LSP 语义 token spans。
///
/// paint 端唯一入口是 [`Self::query_viewport`]；内部 merge 对调用方透明。
pub struct SyntaxHighlights {
    tree: Arc<BufferSyntaxTree>,
    lsp_spans: Option<Arc<Vec<(TextRange, HighlightSpan)>>>,
}

impl SyntaxHighlights {
    pub fn tree(&self) -> &BufferSyntaxTree {
        &self.tree
    }

    /// 在 `viewport` 字节区间上产 highlight spans。
    ///
    /// tree-sitter base + LSP overlay（LSP 在重叠区间胜出）。
    pub fn query_viewport(
        &self,
        viewport: TextRange,
        cursor: &mut SyntaxQueryCursor,
    ) -> Vec<(TextRange, HighlightSpan)> {
        // -- tree-sitter + LSP overlay --
        let tree_spans = self.tree.query_viewport(viewport, cursor);

        let Some(lsp) = &self.lsp_spans else {
            return tree_spans;
        };

        let lsp_in_view: Vec<_> = lsp
            .iter()
            .filter(|(r, _)| {
                r.start().get() < viewport.end().get() && r.end().get() > viewport.start().get()
            })
            .cloned()
            .collect();

        if lsp_in_view.is_empty() {
            return tree_spans;
        }

        overlay_layers(vec![tree_spans, lsp_in_view])
    }
}

impl std::fmt::Debug for SyntaxHighlights {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyntaxHighlights")
            .field("version", &self.tree.version())
            .field("layers", &self.tree.layers.len())
            .field("has_lsp", &self.lsp_spans.is_some())
            .finish_non_exhaustive()
    }
}

// =============================================================================
// SyntaxHighlightsSlot
// =============================================================================

/// 跨线程共享的 [`SyntaxHighlights`] 构造槽。
///
/// tree 和 LSP spans 独立写入——worker 更新 tree 时不动 LSP，LspHost 更新 LSP 时不动 tree。
/// paint 端 `load()` 把二者快照到一个 `Arc<SyntaxHighlights>`。
#[derive(Clone, Default, Debug)]
pub struct SyntaxHighlightsSlot {
    inner: Arc<Mutex<SyntaxHighlightsState>>,
}

#[derive(Default, Debug)]
struct SyntaxHighlightsState {
    tree: Option<Arc<BufferSyntaxTree>>,
    lsp_spans: Option<Arc<Vec<(TextRange, HighlightSpan)>>>,
    lsp_version: Option<BufferVersion>,
}

impl SyntaxHighlightsSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前快照——`None` 表示尚未首次 parse（worker 还在跑 attach）。
    pub fn load(&self) -> Option<Arc<SyntaxHighlights>> {
        let g = self.inner.lock().ok()?;
        let tree = g.tree.as_ref()?.clone();
        Some(Arc::new(SyntaxHighlights {
            tree,
            lsp_spans: g.lsp_spans.clone(),
        }))
    }

    /// LSP token 版本号——给 `pump_lsp_tokens` 快速判断是否要请求新 tokens 而不构建完整 `SyntaxHighlights`。
    pub fn lsp_version(&self) -> Option<BufferVersion> {
        self.inner.lock().ok().and_then(|g| g.lsp_version)
    }

    /// Worker reparse 后写入最新树。
    ///
    /// 只有新版本 ≥ 当前版本才覆盖。
    pub(crate) fn store_tree(&self, tree: BufferSyntaxTree) {
        if let Ok(mut g) = self.inner.lock() {
            let should = match g.tree.as_ref() {
                Some(curr) => curr.version().get() <= tree.version().get(),
                None => true,
            };
            if should {
                g.tree = Some(Arc::new(tree));
            }
        }
    }

    /// LspHost 收到 `semanticTokens/full` 响应后写入解码后的 spans。
    ///
    /// 只有新版本 ≥ 当前 LSP 版本才覆盖。
    pub fn store_lsp(&self, spans: Arc<Vec<(TextRange, HighlightSpan)>>, version: BufferVersion) {
        if let Ok(mut g) = self.inner.lock() {
            let should = match g.lsp_version {
                Some(v) => v.get() <= version.get(),
                None => true,
            };
            if should {
                g.lsp_spans = Some(spans);
                g.lsp_version = Some(version);
            }
        }
    }

    /// 主线程同步编辑入口：克隆当前所有层 tree、对每层逐条调 `Tree::edit`、把新 tree 写回 slot。
    ///
    /// 只推进 tree 字节坐标，不重 parse、不查 query。LSP spans 保持不变——新 LSP 响应到达后会被覆盖。
    ///
    /// 返回 `true` 表示 slot 里原本就有 tree 并被推进。
    pub(crate) fn try_edit(
        &self,
        edits: &[InputEdit],
        new_snapshot: Snapshot,
        new_version: BufferVersion,
    ) -> bool {
        let Ok(mut g) = self.inner.lock() else {
            return false;
        };
        let Some(curr) = g.tree.as_ref().cloned() else {
            return false;
        };
        let new_layers: Vec<SyntaxLayer> = curr
            .layers
            .iter()
            .map(|layer| {
                let mut new_tree = layer.tree.clone();
                for ie in edits {
                    new_tree.edit(ie);
                }
                SyntaxLayer {
                    config: layer.config.clone(),
                    tree: new_tree,
                }
            })
            .collect();
        g.tree = Some(Arc::new(BufferSyntaxTree::layered(
            new_layers,
            new_snapshot,
            new_version,
        )));
        true
    }

    /// detach 时清空全部槽位。
    pub(crate) fn clear(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.tree = None;
            g.lsp_spans = None;
            g.lsp_version = None;
        }
    }

    /// 无条件覆盖 tree——单测用。运行时一律走 [`Self::store_tree`]。
    #[cfg(test)]
    pub(crate) fn store(&self, tree: BufferSyntaxTree) {
        if let Ok(mut g) = self.inner.lock() {
            g.tree = Some(Arc::new(tree));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::payload::{HighlightName, TokenModifiers};
    use crate::syntax::providers::common::{SharedConfig, build_shared_config};
    use std::sync::Arc;
    use tree_sitter::{InputEdit, Language, Parser, Tree};
    use zom_engine::{Buffer, BufferConfig, ByteOffset, TextRange};

    fn rust_config() -> Arc<SharedConfig> {
        let language: Language = tree_sitter_rust::LANGUAGE.into();
        Arc::new(build_shared_config(language, tree_sitter_rust::HIGHLIGHTS_QUERY).unwrap())
    }

    fn parse(config: &SharedConfig, text: &str) -> Tree {
        let mut parser = Parser::new();
        parser.set_language(&config.language).unwrap();
        parser.parse(text.as_bytes(), None).expect("parse 失败")
    }

    #[test]
    fn store_and_load_round_trip() {
        let cfg = rust_config();
        let buffer = Buffer::from_text("fn a() {}".to_string(), BufferConfig::default()).unwrap();
        let snapshot = buffer.snapshot();
        let tree = parse(&cfg, "fn a() {}");
        let slot = SyntaxHighlightsSlot::new();
        assert!(slot.load().is_none());
        assert!(slot.lsp_version().is_none());
        slot.store(BufferSyntaxTree::single(
            cfg.clone(),
            tree,
            snapshot.clone(),
            snapshot.version(),
        ));
        let loaded = slot.load().expect("应当存在快照");
        assert_eq!(loaded.tree().version(), snapshot.version());
        assert_eq!(loaded.tree().layers.len(), 1);
    }

    #[test]
    fn store_if_newer_skips_older() {
        let cfg = rust_config();
        let mut buffer =
            Buffer::from_text("fn a() {}".to_string(), BufferConfig::default()).unwrap();
        let initial_snap = buffer.snapshot();
        let initial_tree = parse(&cfg, "fn a() {}");
        let slot = SyntaxHighlightsSlot::new();

        buffer.insert(zom_engine::ByteOffset::new(9), " ").unwrap();
        let newer_snap = buffer.snapshot();
        let newer_version = newer_snap.version();

        slot.store(BufferSyntaxTree::single(
            cfg.clone(),
            initial_tree.clone(),
            newer_snap,
            newer_version,
        ));

        // worker 端拿"老版本"试图覆盖——必须被丢弃。
        slot.store_tree(BufferSyntaxTree::single(
            cfg,
            initial_tree,
            initial_snap.clone(),
            initial_snap.version(),
        ));
        let loaded = slot.load().unwrap();
        assert_eq!(
            loaded.tree().version(),
            newer_version,
            "更新版本必须保留，过期的 worker reparse 不能覆盖"
        );
    }

    #[test]
    fn store_lsp_only_updates_when_newer() {
        let slot = SyntaxHighlightsSlot::new();
        let spans1: Arc<Vec<_>> = Arc::new(vec![(
            TextRange::new(ByteOffset::new(0), ByteOffset::new(5)).unwrap(),
            HighlightSpan::new(HighlightName::new("keyword"), TokenModifiers::EMPTY),
        )]);
        let v1 = BufferVersion::new(1);
        let v2 = BufferVersion::new(2);

        slot.store_lsp(spans1.clone(), v1);
        assert_eq!(slot.lsp_version(), Some(v1));

        // 旧版本不应覆盖
        slot.store_lsp(Arc::new(Vec::new()), BufferVersion::new(0));
        assert_eq!(slot.lsp_version(), Some(v1));

        // 新版本覆盖
        let spans2: Arc<Vec<_>> = Arc::new(Vec::new());
        slot.store_lsp(spans2, v2);
        assert_eq!(slot.lsp_version(), Some(v2));
    }

    #[test]
    fn try_edit_advances_single_layer_tree() {
        let cfg = rust_config();
        let mut buffer =
            Buffer::from_text("fn a() {}".to_string(), BufferConfig::default()).unwrap();
        let initial_snap = buffer.snapshot();
        let initial_tree = parse(&cfg, "fn a() {}");
        let slot = SyntaxHighlightsSlot::new();
        slot.store(BufferSyntaxTree::single(
            cfg,
            initial_tree,
            initial_snap.clone(),
            initial_snap.version(),
        ));

        buffer.insert(zom_engine::ByteOffset::new(9), " ").unwrap();
        let new_snap = buffer.snapshot();
        let new_version = new_snap.version();
        let edit = InputEdit {
            start_byte: 9,
            old_end_byte: 9,
            new_end_byte: 10,
            start_position: tree_sitter::Point::new(0, 9),
            old_end_position: tree_sitter::Point::new(0, 9),
            new_end_position: tree_sitter::Point::new(0, 10),
        };
        assert!(slot.try_edit(&[edit], new_snap.clone(), new_version));
        let loaded = slot.load().unwrap();
        assert_eq!(loaded.tree().version(), new_version);
        assert_eq!(
            loaded.tree().tree().root_node().end_byte(),
            10,
            "tree.edit 后根节点 end_byte 必须与新 snapshot 对齐"
        );
    }

    #[test]
    fn try_edit_advances_all_layers_byte_range() {
        let cfg = rust_config();
        let mut buffer =
            Buffer::from_text("fn a() {}".to_string(), BufferConfig::default()).unwrap();
        let initial_snap = buffer.snapshot();
        let tree_a = parse(&cfg, "fn a() {}");
        let tree_b = parse(&cfg, "fn a() {}");
        let slot = SyntaxHighlightsSlot::new();
        slot.store(BufferSyntaxTree::layered(
            vec![
                SyntaxLayer {
                    config: cfg.clone(),
                    tree: tree_a,
                },
                SyntaxLayer {
                    config: cfg.clone(),
                    tree: tree_b,
                },
            ],
            initial_snap.clone(),
            initial_snap.version(),
        ));

        buffer.insert(zom_engine::ByteOffset::new(9), " ").unwrap();
        let new_snap = buffer.snapshot();
        let new_version = new_snap.version();
        let edit = InputEdit {
            start_byte: 9,
            old_end_byte: 9,
            new_end_byte: 10,
            start_position: tree_sitter::Point::new(0, 9),
            old_end_position: tree_sitter::Point::new(0, 9),
            new_end_position: tree_sitter::Point::new(0, 10),
        };
        assert!(slot.try_edit(&[edit], new_snap.clone(), new_version));
        let loaded = slot.load().unwrap();
        assert_eq!(loaded.tree().version(), new_version);
        for layer in &loaded.tree().layers {
            assert_eq!(
                layer.tree.root_node().end_byte(),
                10,
                "每层 tree.edit 后 root_end_byte 都必须推进到新 snapshot 末尾"
            );
        }
    }

    #[test]
    fn clear_removes_tree_and_lsp() {
        let cfg = rust_config();
        let buffer = Buffer::from_text("fn a() {}".to_string(), BufferConfig::default()).unwrap();
        let snapshot = buffer.snapshot();
        let tree = parse(&cfg, "fn a() {}");
        let slot = SyntaxHighlightsSlot::new();
        slot.store(BufferSyntaxTree::single(
            cfg,
            tree,
            snapshot.clone(),
            snapshot.version(),
        ));
        let spans: Arc<Vec<_>> = Arc::new(vec![(
            TextRange::new(ByteOffset::new(0), ByteOffset::new(5)).unwrap(),
            HighlightSpan::new(HighlightName::new("keyword"), TokenModifiers::EMPTY),
        )]);
        slot.store_lsp(spans, snapshot.version());
        assert!(slot.load().is_some());
        assert!(slot.lsp_version().is_some());

        slot.clear();
        assert!(slot.load().is_none());
        assert!(slot.lsp_version().is_none());
    }
}
