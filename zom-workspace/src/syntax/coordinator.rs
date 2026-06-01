//! BufferSyntaxState：单个缓冲区的语法高亮调度状态（**异步前台句柄**）。
//!
//! 设计来自《桌面端语法高亮》§七，并按[改造方案 §4.2 / §4.7](../../docs/语法高亮异步增量改造.md) 收口为异步路径。
//!
//! ## 当前形态
//!
//! provider 实例（`Box<dyn HighlightProvider>`）不再挂在 BufferSyntaxState 内，
//! 它在 attach 时被一次性移交给 [`SyntaxWorkerHandle`] 持有的后台线程。本结构体只保留**主线程视角**需要的最小集：
//!
//! - `buffer_id`、`language`：用于任务寻址与诊断；
//! - `sink`：轻量 clone 的 Arc，worker 推产物、主线程在每帧 `pump_pending_highlights`
//!   时 drain；
//! - `worker`：`Arc<SyntaxWorkerHandle>`，发 Attach / Edit / Detach 任务的出口。
//!
//! ## DeltaEvent 单一消费方
//!
//! 调度层**不**自己调 `Buffer::take_pending_events`——那条排他链已被
//! [`crate::WorkspaceBuffer::pump_post_edit`] 占了。pump 入口把每个事件
//! 扇出到 [`BufferSyntaxState::handle_edit`]，本方法只接 ChangeSet 引用与
//! 新版本号，不再自取事件。`handle_edit` 立刻把事件克隆成 `Job::Edit`
//! 投到 worker，**不**当场期待产物落 layer——产物经 sink 异步抵达，主线程下
//! 一帧调 `pump_pending_highlights` 时再落。
//!
//! ## 版本守护（drain 端）
//!
//! drain sink 时按手册 §五 比对版本：
//! - == buffer.version → 落 layer
//! - <  buffer.version → 静默丢（worker 算的是过期版本）
//! - >  buffer.version → debug_assert + 生产丢弃

use std::sync::Arc;

use zom_engine::{Buffer, BufferVersion, ChangeSet, MetadataLayers, TextRange};

use crate::BufferId;

use super::language::LanguageId;
use super::payload::{HighlightSpan, syntax_layer_kind};
use super::provider::HighlightProvider;
use super::sink::{HighlightSink, SinkMessage};
use super::worker::SyntaxWorkerHandle;

/// 大小超此阈值的缓冲区不挂 provider（手册 §十二）。
///
/// **16 MiB**：放阈值的现实平衡点。bench 实测 16 MiB rust 单键 viewport-scoped
/// e2e ≈ 63 ms、主线程 3 µs、cold parse 1.56 s——一项流畅一档可接受；同档
/// 64 MiB 端到端飙到 ~250 ms / 键、cold parse 6.3 s，多出来的部分主要在
/// tree-sitter 解析树本身的 O(file size) 走树代价，不是本项目能撬动的。再大
/// 的文件几乎只剩日志、生成代码、压缩单页 HTML，那些既无语法语义、又应被
/// 宿主提示跳过高亮——超 16 MiB 就静默落到 plain 模式即可。
///
/// 仍是固定常量；不允许 provider 自报阈值。配置项放在 BufferConfig / workspace
/// 设置层是下一步话题。
pub const MAX_HIGHLIGHT_BYTES: usize = 16 * 1024 * 1024;

/// 一个缓冲区在语法高亮子系统里的运行态（主线程句柄）。
///
/// 由 [`BufferSyntaxState::attach`] 创建（Workspace 在 open_file / open_text
/// 后调用），detach 在 buffer 关闭或切换语言时调用。
pub struct BufferSyntaxState {
    buffer_id: BufferId,
    language: LanguageId,
    sink: HighlightSink,
    worker: Arc<SyntaxWorkerHandle>,
}

impl std::fmt::Debug for BufferSyntaxState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferSyntaxState")
            .field("buffer_id", &self.buffer_id)
            .field("language", &self.language)
            .finish_non_exhaustive()
    }
}

impl BufferSyntaxState {
    /// 给一个缓冲区挂上 provider。
    ///
    /// **异步**：本调用只投一条 `Job::Attach` 到 worker 线程；首份产物什么时候落
    /// `layers` 取决于 worker 的处理顺序与主线程下一次 `pump_pending_highlights`。
    /// 调用方在 detect 出非 plain 语言且 `registry.make_provider` 返回实例后才
    /// 调本方法。
    ///
    /// `initial_viewport`：调用方若在 attach 时已知首屏 viewport（典型场景是
    /// desktop 打开文件、活动 view 与 byte range 已就位），传 `Some(range)` 让
    /// worker 在 attach 阶段就只对 viewport 段跑 query 并以 ReplaceRange 投
    /// 产物——viewport-aware attach 把"冷启动高亮亮起"从全树 query 的秒级
    /// 落到 viewport 段的百毫秒级。不知道时传 `None`，行为退化到全文
    /// `ReplaceAll`，后续 `set_viewport_hint` 再异步切换。
    pub fn attach(
        buffer_id: BufferId,
        language: LanguageId,
        provider: Box<dyn HighlightProvider>,
        buffer: &Buffer,
        _layers: &mut MetadataLayers<HighlightSpan>,
        worker: Arc<SyntaxWorkerHandle>,
        initial_viewport: Option<TextRange>,
    ) -> Self {
        let sink = HighlightSink::new();
        worker.attach(
            buffer_id,
            provider,
            buffer.snapshot(),
            sink.clone(),
            initial_viewport,
        );
        Self {
            buffer_id,
            language,
            sink,
            worker,
        }
    }

    pub fn language(&self) -> LanguageId {
        self.language
    }

    pub fn buffer_id(&self) -> BufferId {
        self.buffer_id
    }

    /// 通知 worker 当前 viewport 对应的 byte 区间。
    ///
    /// 由 [`crate::WorkspaceBuffer::set_viewport_hint`] 在 desktop 滚动 / 编辑
    /// 触发 viewport 改变时转发。`None` 表示回退到全文产物（无 viewport 约束）。
    pub fn set_viewport_hint(&self, byte_range: Option<TextRange>) {
        self.worker.set_viewport(self.buffer_id, byte_range);
    }

    /// 编辑发生后调度入口。
    ///
    /// 主线程消耗 = 克隆 Snapshot（rope Arc 共享）+ 克隆 ChangeSet（edits 在
    /// `EditList` 内 `Arc<[Edit]>` 共享）+ 投一次 channel send。这条路径不再
    /// 调 provider，所以**单字符按键的主线程时间与文件大小无关**。
    pub fn handle_edit(
        &self,
        buffer: &Buffer,
        change: &ChangeSet,
        new_version: BufferVersion,
        _layers: &mut MetadataLayers<HighlightSpan>,
    ) {
        self.worker.edit(
            self.buffer_id,
            change.clone(),
            buffer.snapshot(),
            new_version,
        );
    }

    /// 关闭缓冲区 / 切换语言时清空 entry 与 syntax layer。
    ///
    /// 不变量（手册 §九）：syntax layer 在 detach 后清空；不允许「旧高亮残留
    /// + 新高亮叠加」。`Self` 在调用后被消耗。
    ///
    /// `sink.close()` 先置位再投 `Job::Detach`：worker 处理 detach 前的任何还
    /// 在飞的 Edit 即便算完产物也会被 sink 自身的 closed 闸门拦下，不污染下一任
    /// provider 的 layer。
    pub fn detach(self, layers: &mut MetadataLayers<HighlightSpan>) {
        self.sink.close();
        self.worker.detach(self.buffer_id);
        let kind = syntax_layer_kind();
        if let Some(existing) = layers.layer(&kind) {
            let version = existing.version();
            let _ = layers.replace_layer_ranges(
                kind,
                version,
                std::iter::empty::<(zom_engine::TextRange, HighlightSpan)>(),
            );
        }
    }

    /// 把 sink 中已就绪的产物落到 layers——每帧 prepaint 由
    /// [`crate::Workspace::pump_pending_highlights`] 统一驱动。
    ///
    /// 不阻塞、不等待 worker；sink 为空就立即返回。
    ///
    /// ## coalesce 与 ReplaceRange 消费
    ///
    /// drain 出的消息序列里允许混杂 [`SinkMessage::ReplaceAll`] 与
    /// [`SinkMessage::ReplaceRange`]：
    ///
    /// - 遇到一条 `ReplaceAll`：它直接重建整层 spans，之前累积的 `ReplaceAll` 与
    ///   `ReplaceRange` 都被它覆盖——清空累计；
    /// - 累计期遇到 `ReplaceRange`：先 stash，等收集完所有消息后按 FIFO 顺序应用
    ///   到 `MetadataLayers::replace_layer_ranges_in_range`（[改造方案 §4.6](
    ///   ../../docs/语法高亮异步增量改造.md)）；
    ///
    /// 每条消息都独立做版本守护（手册 §五）：
    /// - == buffer.version → 落 layer；
    /// - <  buffer.version → 静默丢（worker 算的是过期版本，下一轮 on_edit 会重算）；
    /// - >  buffer.version → debug_assert，生产丢弃；整条 batch 中断。
    pub(crate) fn drain_into_layers(
        &self,
        current_version: BufferVersion,
        layers: &mut MetadataLayers<HighlightSpan>,
    ) {
        let messages = self.sink.drain();
        if messages.is_empty() {
            return;
        }

        let mut anchor: Option<(BufferVersion, Vec<(zom_engine::TextRange, HighlightSpan)>)> = None;
        let mut ranges: Vec<(
            BufferVersion,
            zom_engine::TextRange,
            Vec<(zom_engine::TextRange, HighlightSpan)>,
        )> = Vec::new();
        for msg in messages {
            match msg {
                SinkMessage::ReplaceAll { version, spans } => {
                    anchor = Some((version, spans));
                    ranges.clear();
                }
                SinkMessage::ReplaceRange {
                    version,
                    range,
                    spans,
                } => {
                    ranges.push((version, range, spans));
                }
            }
        }

        let kind = syntax_layer_kind();
        if let Some((version, spans)) = anchor {
            if !version_landed(version, current_version) {
                return;
            }
            let _ = layers.replace_layer_ranges(kind.clone(), version, spans);
        }
        for (version, byte_range, spans) in ranges {
            if !version_landed(version, current_version) {
                continue;
            }
            let _ = layers.replace_layer_ranges_in_range(kind.clone(), version, byte_range, spans);
        }
    }
}

/// 版本守护：` == ` 落 layer；` < ` 过期丢弃；` > ` debug_assert 后丢弃。
///
/// `>` 情况返回 `false`——调用方在 anchor 路径上视为整条 batch 中断，因为下面
/// 任何 ReplaceRange 都会基于这个「未来版本」产生不一致；ReplaceRange 路径上则
/// 仅丢单条，继续处理后续同 batch 的 ReplaceRange——它们独立做版本判定。
fn version_landed(msg_version: BufferVersion, current_version: BufferVersion) -> bool {
    if msg_version.get() > current_version.get() {
        debug_assert!(
            false,
            "高亮提供程序生成的版本 {} > 缓冲区版本 {}",
            msg_version.get(),
            current_version.get()
        );
        return false;
    }
    msg_version.get() == current_version.get()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::payload::HighlightName;
    use crate::syntax::provider::BufferHandle;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use zom_engine::{BufferConfig, ByteOffset, TextRange};

    /// 一个在 attach 时推一份固定 span 的 mock provider。
    struct StaticProvider {
        attached_count: Arc<AtomicUsize>,
        detached_count: Arc<AtomicUsize>,
    }

    impl HighlightProvider for StaticProvider {
        fn language(&self) -> LanguageId {
            LanguageId::new("rust")
        }
        fn attach(&mut self, buffer: BufferHandle, sink: HighlightSink) {
            self.attached_count.fetch_add(1, Ordering::SeqCst);
            let snap = buffer.snapshot();
            sink.replace_all(
                snap.version(),
                vec![(
                    TextRange::new(ByteOffset::new(0), ByteOffset::new(2)).unwrap(),
                    HighlightSpan::from_name(HighlightName::new("keyword")),
                )],
            );
        }
        fn on_edit(&mut self, _buffer: BufferHandle, _change: &ChangeSet, _version: BufferVersion) {
        }
        fn detach(&mut self) {
            self.detached_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn make_buffer(text: &str) -> Buffer {
        Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
    }

    fn make_provider() -> (
        Box<dyn HighlightProvider>,
        Arc<AtomicUsize>,
        Arc<AtomicUsize>,
    ) {
        let attached = Arc::new(AtomicUsize::new(0));
        let detached = Arc::new(AtomicUsize::new(0));
        let provider = Box::new(StaticProvider {
            attached_count: attached.clone(),
            detached_count: detached.clone(),
        });
        (provider, attached, detached)
    }

    fn id(n: u64) -> BufferId {
        // 测试用：构造任意 BufferId
        BufferId::from_raw(n)
    }

    #[test]
    fn attach_lays_initial_spans_after_pump() {
        let (provider, attached, _) = make_provider();
        let buffer = make_buffer("fn main() {}");
        let mut layers = MetadataLayers::<HighlightSpan>::new();
        let worker = Arc::new(SyntaxWorkerHandle::spawn());
        let state = BufferSyntaxState::attach(
            id(1),
            LanguageId::new("rust"),
            provider,
            &buffer,
            &mut layers,
            worker,
            None,
        );
        // 异步：等 worker 算完后再 drain。
        state.worker.wait_for_idle();
        state.drain_into_layers(buffer.version(), &mut layers);

        assert_eq!(attached.load(Ordering::SeqCst), 1);
        let layer = layers.layer(&syntax_layer_kind()).expect("layer 必须存在");
        assert_eq!(layer.len(), 1);
        assert_eq!(state.language(), LanguageId::new("rust"));
    }

    #[test]
    fn detach_clears_layer() {
        let (provider, _, detached) = make_provider();
        let buffer = make_buffer("fn main() {}");
        let mut layers = MetadataLayers::<HighlightSpan>::new();
        let worker = Arc::new(SyntaxWorkerHandle::spawn());
        let state = BufferSyntaxState::attach(
            id(2),
            LanguageId::new("rust"),
            provider,
            &buffer,
            &mut layers,
            worker.clone(),
            None,
        );
        worker.wait_for_idle();
        state.drain_into_layers(buffer.version(), &mut layers);
        assert!(layers.layer(&syntax_layer_kind()).unwrap().len() > 0);

        state.detach(&mut layers);
        worker.wait_for_idle();

        assert_eq!(detached.load(Ordering::SeqCst), 1);
        let layer = layers.layer(&syntax_layer_kind()).expect("layer 必须存在");
        assert_eq!(layer.len(), 0, "detach 后 syntax layer 应为空");
    }

    #[test]
    fn drain_drops_future_version() {
        let (provider, _, _) = make_provider();
        let buffer = make_buffer("fn main() {}");
        let mut layers = MetadataLayers::<HighlightSpan>::new();
        let worker = Arc::new(SyntaxWorkerHandle::spawn());
        let state = BufferSyntaxState::attach(
            id(3),
            LanguageId::new("rust"),
            provider,
            &buffer,
            &mut layers,
            worker.clone(),
            None,
        );
        worker.wait_for_idle();
        state.drain_into_layers(buffer.version(), &mut layers);

        let original_len = layers.layer(&syntax_layer_kind()).unwrap().len();
        let future = BufferVersion::new(buffer.version().get() + 999);
        state.sink.replace_all(
            future,
            vec![(
                TextRange::new(ByteOffset::new(0), ByteOffset::new(1)).unwrap(),
                HighlightSpan::from_name(HighlightName::new("string")),
            )],
        );

        // 发布构建不触发 panic；调试构建下 catch_unwind 兜底，让两种构建都通过。
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            state.drain_into_layers(buffer.version(), &mut layers);
        }));
        assert_eq!(
            layers.layer(&syntax_layer_kind()).unwrap().len(),
            original_len,
            "future version must not overwrite layer"
        );
    }
}
