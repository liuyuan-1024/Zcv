//! HighlightSink：provider → 调度层的产物投递通道。
//!
//! 设计来自《桌面端语法高亮》§五。
//!
//! - **轻量 clone**：内部 `Arc<Mutex<SinkInner>>`；任何线程拿到的 sink 句柄
//!   都能推产物。
//! - **写入侧不直接动 [`zom_engine::MetadataLayers`]**：sink 把 `replace_all` /
//!   `replace_range` 调用记下来排入队列；coordinator 调
//!   [`HighlightSink::drain`] 把队列原子取出再统一写回。这样：
//!     - 同步 provider：on_edit 返回后立即 drain，一次推完一次写完。
//!     - 异步 provider（如 LSP）：server 回包推到 sink，主线程在合适时机 drain，
//!       不需要跨线程持有 `&mut WorkspaceBuffer`。
//! - **版本对齐与 coalesce 在 drain 端做**：sink 只负责忠实排队，coordinator
//!   做版本守护与同 buffer 多份 replace_all 折叠（手册 §五 / §七.4）。

use std::sync::{Arc, Mutex};

use zom_engine::{BufferVersion, TextRange};

use super::payload::HighlightSpan;

/// provider 推产物的句柄。Clone 仅复制内部 Arc。
#[derive(Clone, Debug)]
pub struct HighlightSink {
    inner: Arc<Mutex<SinkInner>>,
}

#[derive(Debug, Default)]
struct SinkInner {
    pending: Vec<SinkMessage>,
    /// detach 后置位；之后所有 push 静默丢弃。手册 §四 / §八 要求 detach
    /// 返回后绝不再有产物落进调度层；异步 provider 内部如果晚到了一拍，sink
    /// 这一关也能兜住，避免污染下一任 provider 的 layer。
    closed: bool,
}

/// sink 中一次产物投递。
///
/// `Replace*` 两个变体对应 [`HighlightSink`] 上的两个方法；coordinator drain
/// 时按 FIFO 顺序处理，但同 buffer 的 `ReplaceAll` 会被折叠到最后一条
/// （手册 §七.4 的写入侧 coalesce）——一次 drain 调用内只对最后一个
/// `ReplaceAll` 之后的 `ReplaceRange` 生效。
#[derive(Debug)]
pub enum SinkMessage {
    ReplaceAll {
        version: BufferVersion,
        spans: Vec<(TextRange, HighlightSpan)>,
    },
    ReplaceRange {
        version: BufferVersion,
        range: TextRange,
        spans: Vec<(TextRange, HighlightSpan)>,
    },
}

impl HighlightSink {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SinkInner::default())),
        }
    }

    /// detach 时由 coordinator 调。置位后任何 push 静默丢弃，drain 也只能
    /// 取走已在队列里的、调用前推入的产物——本次 detach 后任何晚到的产物
    /// 都不会跨越 detach 边界落到下一任 provider 的 layer 上。
    pub(crate) fn close(&self) {
        let mut guard = self.inner.lock().expect("HighlightSink 互斥锁中毒");
        guard.closed = true;
    }

    /// coordinator 调：原子取出所有待落产物。
    pub(crate) fn drain(&self) -> Vec<SinkMessage> {
        let mut guard = self.inner.lock().expect("HighlightSink 互斥锁中毒");
        std::mem::take(&mut guard.pending)
    }

    /// 全量替换：用新 snapshot 覆盖该 buffer 的整个 syntax layer。
    pub fn replace_all(&self, version: BufferVersion, spans: Vec<(TextRange, HighlightSpan)>) {
        let mut guard = self.inner.lock().expect("HighlightSink 互斥锁中毒");
        if guard.closed {
            return;
        }
        guard
            .pending
            .push(SinkMessage::ReplaceAll { version, spans });
    }

    /// 局部替换：覆盖给定区间内的所有 span，区间外保持不变。
    /// tree-sitter 增量 / LSP delta 用；coordinator 会按版本守护消费局部产物。
    pub fn replace_range(
        &self,
        version: BufferVersion,
        range: TextRange,
        spans: Vec<(TextRange, HighlightSpan)>,
    ) {
        let mut guard = self.inner.lock().expect("HighlightSink 互斥锁中毒");
        if guard.closed {
            return;
        }
        guard.pending.push(SinkMessage::ReplaceRange {
            version,
            range,
            spans,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zom_engine::ByteOffset;

    fn span() -> HighlightSpan {
        HighlightSpan::from_name(super::super::payload::HighlightName::new("keyword"))
    }

    fn range(a: usize, b: usize) -> TextRange {
        TextRange::new(ByteOffset::new(a), ByteOffset::new(b)).unwrap()
    }

    #[test]
    fn drain_returns_messages_in_push_order() {
        let sink = HighlightSink::new();
        sink.replace_all(BufferVersion::new(1), vec![(range(0, 3), span())]);
        sink.replace_range(
            BufferVersion::new(1),
            range(0, 10),
            vec![(range(3, 5), span())],
        );
        let msgs = sink.drain();
        assert_eq!(msgs.len(), 2);
        assert!(matches!(msgs[0], SinkMessage::ReplaceAll { .. }));
        assert!(matches!(msgs[1], SinkMessage::ReplaceRange { .. }));
        // 再次 drain 应空
        assert!(sink.drain().is_empty());
    }

    #[test]
    fn close_drops_subsequent_pushes() {
        let sink = HighlightSink::new();
        sink.replace_all(BufferVersion::new(1), vec![(range(0, 3), span())]);
        sink.close();
        // close 之后的 push 静默丢弃；close 之前的 push 仍可被 drain。
        sink.replace_all(BufferVersion::new(2), vec![(range(0, 3), span())]);
        let msgs = sink.drain();
        assert_eq!(msgs.len(), 1);
        match &msgs[0] {
            SinkMessage::ReplaceAll { version, .. } => assert_eq!(version.get(), 1),
            _ => panic!("期望 ReplaceAll 消息"),
        }
    }
}
