//! HighlightProvider trait + BufferHandle。
//!
//! 计划 §Phase 3 后形态：provider 只剩"持有 Parser + Tree、按编辑事件 reparse、把 (config + tree + snapshot + version) 导出到共享 [`BufferSyntaxTreeSlot`]"四步。
//! sink、viewport hint、apply_pending_edit / coalesce 全部下线 —— viewport-scoped Query 由 paint 阶段（[`crate::syntax::BufferSyntaxTree::query_viewport`]）现查；
//! coalesce 也不再需要 —— 产物落在 slot 而不是 sink 队列，下一次 reparse 自然覆盖。
//!
//! trait 仍存在的理由：把 tree-sitter / LSP / 占位三类 provider 形态对调度层 ([`crate::syntax::worker`]) 抽象成同一面；
//! 调度层负责 Job 调度、panic 隔离、Entry 生命周期，不关心 provider 装的是哪门语言。

use std::sync::{Arc, RwLock};

use zom_engine::{BufferVersion, ChangeSet, Snapshot};

use super::language::LanguageId;
use super::tree::BufferSyntaxTreeSlot;

/// 调度层借给 provider 的 buffer 只读句柄。
///
/// **轻量 clone**：内部 `Arc<RwLock<Snapshot>>`。
/// Provider 可以跨线程持有，但**绝不能**自己持有真正的 `Buffer`——读取一律走 `snapshot()`。
#[derive(Clone, Debug)]
pub struct BufferHandle {
    inner: Arc<RwLock<Snapshot>>,
}

impl BufferHandle {
    pub(crate) fn new(snapshot: Snapshot) -> Self {
        Self {
            inner: Arc::new(RwLock::new(snapshot)),
        }
    }

    /// 取当前版本的 snapshot 克隆。rope 内部共享，开销极低。
    pub fn snapshot(&self) -> Snapshot {
        self.inner
            .read()
            .expect("BufferHandle snapshot 读锁中毒")
            .clone()
    }

    /// 当前版本号——provider 想做"先看版本再决定是否要跑"的快路径时用。
    pub fn version(&self) -> BufferVersion {
        self.inner
            .read()
            .expect("BufferHandle snapshot 读锁中毒")
            .version()
    }
}

/// 语法高亮 provider 的最小公共面（Phase 3 后形态）。
pub trait HighlightProvider: Send + Sync {
    /// 本 provider 服务的语言 id。registry 在 attach 前已经按此 id 选了 provider，
    /// 但留这个方法是给后续多 provider 叠加 / 诊断显示用。
    fn language(&self) -> LanguageId;

    /// 把一个 buffer 挂上来开始供应高亮——provider 在此调用内做首次 parse。
    fn attach(&mut self, buffer: BufferHandle);

    /// buffer 发生编辑后调度层通知 provider。
    /// tree-sitter provider 在此调用内做增量 reparse；
    /// LSP provider 只更新内部版本号，真正推送来自 server 异步回包（未来形态）。
    ///
    /// `version` = 本次编辑后的新版本，等于 `buffer.version()`。
    fn on_edit(&mut self, buffer: BufferHandle, change: &ChangeSet, version: BufferVersion);

    /// buffer 关闭 / 切换语言 / provider 卸载时调度层调用。
    /// Provider 必须释放与该 buffer 相关的全部资源；
    /// 调用返回后**不得**再向 slot 写任何内容。
    fn detach(&mut self);

    /// 把 provider 当前持有的"语法树 + 对应 snapshot"导出到共享槽位。
    ///
    /// 调度层（[`crate::syntax::worker`]）在每次 attach / on_edit 处理完成后调用，
    /// 让 paint 阶段能用 [`BufferSyntaxTreeSlot::load`] 拿到与 worker 内部状态对齐的 [`crate::syntax::BufferSyntaxTree`]。
    ///
    /// 默认实现无操作——LSP / 占位 provider 没有 tree-sitter 树，自然没什么可导出。
    /// tree-sitter provider 在自身的 `HighlightWorker` 上 override。
    fn export_syntax_tree(&self, slot: &BufferSyntaxTreeSlot) {
        let _ = slot;
    }
}
