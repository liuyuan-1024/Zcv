//! `BufferSyntaxTree`：单缓冲区的"当前 tree-sitter 解析树 + 对应 snapshot"共享态。
//!
//! ## 角色
//!
//! 把"语言配置 + 当前 Tree + 对应 Snapshot + 版本号"打包成一个**不可变快照**值，
//! 通过 [`BufferSyntaxTreeSlot`] 在主线程与后台 SyntaxWorker 之间共享：
//!
//! - **worker 线程**（[`crate::syntax::worker`]）在 attach / 增量 reparse 完成后用 [`BufferSyntaxTreeSlot::store_if_newer`] 写入最新树；
//! - **主线程**编辑入口（[`crate::syntax::BufferSyntax::handle_edit`]）在每次编辑发生时用 [`BufferSyntaxTreeSlot::try_edit`] 把 `tree.edit(InputEdit)` 同步推进到 slot 里 —— 让 paint 阶段哪怕在 worker 还没回包前也能看见**带正确字节坐标**的 tree；
//! - **paint 阶段**用 [`BufferSyntaxTreeSlot::load`] 拿到 `Arc<BufferSyntaxTree>`，按 viewport 现查 tree-sitter Query 出 spans。
//!
//! ## 为什么 Mutex
//!
//! tree-sitter 的 `Tree::clone` 本身是 `O(1)`（内部 `Arc` 共享），加上"两侧都要写"的现实（主线程 tree.edit + worker reparse），单写多读的 `arc_swap` 范式并不直接适用。
//!
//! 锁的临界区只包**载 / 存 Arc**，最长一次 `Tree::clone` + `tree.edit(InputEdit)`，都是 `O(log N)` 操作；
//! 锁竞争窗口比"主线程 paint 内的整段 query"短两个量级，不构成拖帧来源。

use std::sync::{Arc, Mutex};

use tree_sitter::{InputEdit, QueryCursor, Tree};
use zom_engine::{BufferVersion, Snapshot, TextRange};

use super::payload::HighlightSpan;
use super::providers::common::{
    SharedConfig, SnapshotTextProvider, collect_spans, reset_cursor_range,
};

/// 缓冲区当前的语法树快照——`config` + `tree` + `snapshot` + `version` 一致。
///
/// 不可变值类型：每次更新都用 `Arc<BufferSyntaxTree>` 整体替换。
/// `Tree::clone` 是 `O(1)`（tree-sitter 内部 `Arc` 共享），所以"克隆一份再 `tree.edit`"的代价只在 `tree.edit` 自身的 `O(log N)` 字节坐标推进上。
pub struct BufferSyntaxTree {
    /// 语言级共享配置（query + capture lookup）。每条语言一份，跨 buffer 复用。
    pub(crate) config: Arc<SharedConfig>,
    /// 当前已知的最新 tree。可能是 worker reparse 出来的"结构正确的 tree"，也可能是主线程 `tree.edit` 推进过坐标但**还没 reparse**的 interpolate tree。
    /// paint 阶段直接拿它跑 query；query 命中的 node 坐标永远与 `snapshot` 对齐。
    pub(crate) tree: Tree,
    /// 与 `tree` 对应的 buffer snapshot。`SnapshotTextProvider` 走它读节点文本。
    pub(crate) snapshot: Snapshot,
    /// `snapshot.version()` 缓存——store 时比版本时少一次 snapshot 解引用。
    pub(crate) version: BufferVersion,
}

impl BufferSyntaxTree {
    pub(crate) fn new(
        config: Arc<SharedConfig>,
        tree: Tree,
        snapshot: Snapshot,
        version: BufferVersion,
    ) -> Self {
        Self {
            config,
            tree,
            snapshot,
            version,
        }
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn version(&self) -> BufferVersion {
        self.version
    }

    /// 在 `viewport` 字节区间上跑 tree-sitter Query，返回非重叠 `(range, span)` 列表。
    ///
    /// paint 阶段的"render-time query"入口：不缓存 spans、每帧重算，颜色 = 当前 tree 在 viewport 上的纯函数。
    ///
    /// **复用 `cursor`**：tree-sitter 的 [`QueryCursor`] 构造一次还好，每帧多次会累积分配；
    /// 调用方应当用 [`SyntaxQueryCursor`] 在 thread-local / surface 上缓存一份，每帧 paint 时借出。
    ///
    /// query 命中的字节范围一律落在 `viewport` 内（`set_byte_range`），返回后 cursor 被复位到全文范围，可立即用于下一次 query 而不会被上一次约束截断。
    pub fn query_viewport(
        &self,
        viewport: TextRange,
        cursor: &mut SyntaxQueryCursor,
    ) -> Vec<(TextRange, HighlightSpan)> {
        let inner = &mut cursor.inner;
        inner.set_byte_range(viewport.start().get()..viewport.end().get());
        let provider = SnapshotTextProvider {
            snapshot: &self.snapshot,
        };
        let spans = collect_spans(&self.config, inner, &self.tree, provider);
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
            .field("snapshot_len", &self.snapshot.len_bytes().get())
            .finish_non_exhaustive()
    }
}

/// 跨线程共享的 [`BufferSyntaxTree`] 槽位。轻量 clone（内部 `Arc<Mutex>`）。
///
/// 主线程持一份用于 `try_edit` 与 `load`；worker 线程持一份用于 `store_if_newer`。
#[derive(Clone, Default, Debug)]
pub struct BufferSyntaxTreeSlot {
    inner: Arc<Mutex<Option<Arc<BufferSyntaxTree>>>>,
}

impl BufferSyntaxTreeSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// 当前快照——`None` 表示尚未首次 parse（worker 还在跑 attach）。
    ///
    /// 返回 `Arc<BufferSyntaxTree>` clone（原子计数 +1），调用方拿走后立即放锁。
    pub fn load(&self) -> Option<Arc<BufferSyntaxTree>> {
        self.inner.lock().ok().and_then(|g| g.as_ref().cloned())
    }

    /// 无条件覆盖——单测用。运行时一律走 [`Self::store_if_newer`]，避免把过期 reparse 结果盖掉。
    #[cfg(test)]
    pub(crate) fn store(&self, tree: BufferSyntaxTree) {
        if let Ok(mut g) = self.inner.lock() {
            *g = Some(Arc::new(tree));
        }
    }

    /// 只有"新版本 ≥ 当前版本"才覆盖——避免 worker 把过期 reparse 结果盖到主线程已经 `tree.edit` 推进过的更新版本上。
    pub(crate) fn store_if_newer(&self, tree: BufferSyntaxTree) {
        if let Ok(mut g) = self.inner.lock() {
            let should = match g.as_ref() {
                Some(curr) => curr.version.get() <= tree.version.get(),
                None => true,
            };
            if should {
                *g = Some(Arc::new(tree));
            }
        }
    }

    /// detach 时清空——保证下一任 provider 不会读到旧 tree。
    pub(crate) fn clear(&self) {
        if let Ok(mut g) = self.inner.lock() {
            *g = None;
        }
    }

    /// 主线程同步编辑入口：克隆当前 tree、逐条调 `Tree::edit`、把新版本写回 slot。
    ///
    /// 不重 parse、不查 query —— 只让 paint 这一帧拿到的 tree 字节坐标与新 snapshot 对齐。
    /// worker 端的 Job::Edit 在后续到达，会用真正的 reparse 结果覆盖本次 interpolate tree。
    ///
    /// 返回 `true` 表示 slot 里原本就有 tree 并被推进；
    /// `false` 表示槽位为空，调用方什么也不必做（worker 的首份 Attach 产物到达后会自动初始化）。
    pub(crate) fn try_edit(
        &self,
        edits: &[InputEdit],
        new_snapshot: Snapshot,
        new_version: BufferVersion,
    ) -> bool {
        let Ok(mut g) = self.inner.lock() else {
            return false;
        };
        let Some(curr) = g.as_ref().cloned() else {
            return false;
        };
        let mut new_tree = curr.tree.clone();
        for ie in edits {
            new_tree.edit(ie);
        }
        *g = Some(Arc::new(BufferSyntaxTree::new(
            curr.config.clone(),
            new_tree,
            new_snapshot,
            new_version,
        )));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::providers::common::build_shared_config;
    use tree_sitter::{Language, Parser};
    use zom_engine::{Buffer, BufferConfig};

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
        let slot = BufferSyntaxTreeSlot::new();
        assert!(slot.load().is_none());
        slot.store(BufferSyntaxTree::new(
            cfg.clone(),
            tree,
            snapshot.clone(),
            snapshot.version(),
        ));
        let loaded = slot.load().expect("应当存在快照");
        assert_eq!(loaded.version(), snapshot.version());
    }

    #[test]
    fn store_if_newer_skips_older() {
        let cfg = rust_config();
        let mut buffer =
            Buffer::from_text("fn a() {}".to_string(), BufferConfig::default()).unwrap();
        let initial_snap = buffer.snapshot();
        let initial_tree = parse(&cfg, "fn a() {}");
        let slot = BufferSyntaxTreeSlot::new();

        // 推进 buffer 到新版本，造一份"主线程已经 tree.edit 过"的更新快照。
        buffer.insert(zom_engine::ByteOffset::new(9), " ").unwrap();
        let newer_snap = buffer.snapshot();
        let newer_version = newer_snap.version();

        slot.store(BufferSyntaxTree::new(
            cfg.clone(),
            initial_tree.clone(),
            newer_snap,
            newer_version,
        ));

        // worker 端拿"老版本"试图覆盖——必须被丢弃。
        slot.store_if_newer(BufferSyntaxTree::new(
            cfg,
            initial_tree,
            initial_snap.clone(),
            initial_snap.version(),
        ));
        let loaded = slot.load().unwrap();
        assert_eq!(
            loaded.version(),
            newer_version,
            "更新版本必须保留，过期的 worker reparse 不能覆盖"
        );
    }

    #[test]
    fn try_edit_advances_tree_byte_range() {
        let cfg = rust_config();
        let mut buffer =
            Buffer::from_text("fn a() {}".to_string(), BufferConfig::default()).unwrap();
        let initial_snap = buffer.snapshot();
        let initial_tree = parse(&cfg, "fn a() {}");
        let slot = BufferSyntaxTreeSlot::new();
        slot.store(BufferSyntaxTree::new(
            cfg,
            initial_tree,
            initial_snap.clone(),
            initial_snap.version(),
        ));

        // 末尾插入一个字符：构造 InputEdit 推进 tree。
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
        assert_eq!(loaded.version(), new_version);
        assert_eq!(
            loaded.tree.root_node().end_byte(),
            10,
            "tree.edit 后根节点 end_byte 必须与新 snapshot 对齐"
        );
    }
}
