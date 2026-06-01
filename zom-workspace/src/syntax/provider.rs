//! HighlightProvider trait + BufferHandle。
//!
//! 设计来自《桌面端语法高亮》§四。trait 是 tree-sitter / LSP / 占位等所有
//! provider 形态的**最小公共面**——不假设语言学语义，只规定生命周期与产物
//! 投递方式。

use std::sync::{Arc, RwLock};

use zom_engine::{BufferVersion, ChangeSet, Snapshot, TextRange};

use super::language::LanguageId;
use super::sink::HighlightSink;

/// 调度层借给 provider 的 buffer 只读句柄。
///
/// **轻量 clone**：内部 `Arc<RwLock<Snapshot>>`。Provider 可以跨线程持有，
/// 但**绝不能**自己持有真正的 `Buffer`——读取一律走 `snapshot()`。
///
/// 调度层在每次调 `attach` / `on_edit` 前用 `coordinator` 私有接口刷新内部
/// snapshot；provider 在调用内拿到的就是当前版本。
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

/// 语法高亮 provider 的最小公共面（手册 §四）。
///
/// **订阅模型**，不是拉模型——`on_edit` 不返回值；产物始终走 `sink`。这样
/// tree-sitter（同步算完即推）与 LSP（server 异步回包再推）能共用一套 trait。
pub trait HighlightProvider: Send + Sync {
    /// 本 provider 服务的语言 id。registry 在 attach 前已经按此 id 选了 provider，
    /// 但留这个方法是给后续多 provider 叠加 / 诊断显示用。
    fn language(&self) -> LanguageId;

    /// 把一个 buffer 挂上来开始供应高亮。
    ///
    /// Provider 自行决定首次产物什么时候、以什么粒度推 sink。
    /// 同步 provider 可在此调用内立即推 sink。
    fn attach(&mut self, buffer: BufferHandle, sink: HighlightSink);

    /// buffer 发生编辑后调度层通知 provider。
    ///
    /// - tree-sitter provider 可在此调用内同步算完并推 sink。
    /// - LSP provider 只更新内部版本号，真正推送来自 server 异步回包。
    ///
    /// `version` = 本次编辑后的新版本，等于 `buffer.version()`。
    fn on_edit(&mut self, buffer: BufferHandle, change: &ChangeSet, version: BufferVersion);

    /// 应用一次"中间编辑"——仅推进 provider 内部状态（如 tree-sitter
    /// `Tree::edit`），**不**重 parse、不 query、不推 sink。
    ///
    /// 调度层在 channel 中发现同 buffer 连续多个编辑事件时，会用本方法处理除最
    /// 后一条外的所有事件，把 N 次按键合并成「**一次 reparse + 一次 query +
    /// 一次 sink push**」，避开中间产物被立刻覆盖的浪费。调用方契约：紧跟着
    /// 必须有一次 `on_edit` 传入最终状态的 buffer / change / version，否则
    /// provider 内部跟踪的 tree 与外部 snapshot 会错位。
    ///
    /// 默认实现回退到 `on_edit`——不支持批量的 provider（LSP / 占位）仍然
    /// 行为正确，只是没有省。tree-sitter 系会在
    /// `crate::syntax::providers::common::HighlightWorker` 里 override 为
    /// 「仅 `Tree::edit`」的快路径。
    fn apply_pending_edit(
        &mut self,
        buffer: BufferHandle,
        change: &ChangeSet,
        version: BufferVersion,
    ) {
        self.on_edit(buffer, change, version);
    }

    /// buffer 关闭 / 切换语言 / provider 卸载时调度层调用。
    ///
    /// Provider 必须释放与该 buffer 相关的全部资源；调用返回后**不得**再向
    /// sink 推任何内容（异步 provider 在自己的 task 里检查 detach 标记）。
    fn detach(&mut self);

    /// 通知 provider 当前 viewport 对应的 byte 区间（可由 desktop 在滚动时
    /// 频繁更新）。`None` 表示取消 viewport 限定，回退到全文产物模式。
    ///
    /// 默认实现无操作：LSP / 占位 provider 等不需要 viewport hint 的 producer
    /// 直接忽略即可。tree-sitter 系（[`crate::syntax::providers`]）会把它接到
    /// `QueryCursor::set_byte_range`，让每次编辑只产 viewport ± 缓冲区段的
    /// `ReplaceRange`（[改造方案 §4.6](
    /// ../../docs/语法高亮异步增量改造.md)）。调用方可在 hint 改变时立即触发
    /// 一次"重 query"（不重 parse），把新区域的 spans 1–2 帧内补齐。
    fn set_viewport(&mut self, byte_range: Option<TextRange>) {
        let _ = byte_range;
    }
}
