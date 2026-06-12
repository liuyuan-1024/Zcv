//! `BufferSyntax`：单个缓冲区的语法高亮**主线程句柄**。
//!
//! 只做两件事：
//!
//! 1. 持有共享的 [`BufferSyntaxTreeSlot`] —— paint 端按它现查 viewport-scoped Query。
//! 2. 在编辑入口同步推进 slot 里 tree 的字节坐标 + 投 worker reparse Job。

use std::sync::Arc;

use zom_engine::{Buffer, DeltaEvent};

use crate::BufferId;

use super::language::LanguageId;
use super::provider::HighlightProvider;
use super::providers::common::translate_edits;
use super::tree::BufferSyntaxTreeSlot;
use super::worker::SyntaxWorkerHandle;

/// 大小超此阈值的缓冲区不挂 provider（手册 §十二）。
///
/// **16 MiB**：放阈值的现实平衡点。
/// bench 实测 16 MiB rust 单键 viewport-scoped e2e ≈ 63 ms、主线程 3 µs、cold parse 1.56 s——一项流畅一档可接受；
/// 再大的文件几乎只剩日志、生成代码——超 16 MiB 就静默落到 plain 模式。
pub const MAX_HIGHLIGHT_BYTES: usize = 16 * 1024 * 1024;

/// 一个缓冲区在语法高亮子系统里的运行态（主线程句柄）。
///
/// 由 [`BufferSyntax::attach`] 创建，detach 在 buffer 关闭 / 切换语言时调用。
pub struct BufferSyntax {
    buffer_id: BufferId,
    language: LanguageId,
    tree_slot: BufferSyntaxTreeSlot,
    worker: Arc<SyntaxWorkerHandle>,
}

impl std::fmt::Debug for BufferSyntax {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferSyntax")
            .field("buffer_id", &self.buffer_id)
            .field("language", &self.language)
            .finish_non_exhaustive()
    }
}

impl BufferSyntax {
    /// 把一个 buffer 挂上 provider。
    ///
    /// 投一条 `Job::Attach` 给 worker 跑首次 parse；
    /// 首份 `BufferSyntaxTree` 落到 slot 的时机取决于 worker 处理顺序。
    /// 在那之前 paint 端 `slot.load()` 返回 `None`，不产生 syntax decoration（buffer 显示为默认前景色）。
    pub fn attach(
        buffer_id: BufferId,
        language: LanguageId,
        provider: Box<dyn HighlightProvider>,
        buffer: &Buffer,
        worker: Arc<SyntaxWorkerHandle>,
    ) -> Self {
        let tree_slot = BufferSyntaxTreeSlot::new();
        worker.attach(buffer_id, provider, buffer.snapshot(), tree_slot.clone());
        Self {
            buffer_id,
            language,
            tree_slot,
            worker,
        }
    }

    /// 主线程读 paint 入口——返回与本缓冲区共享的 [`BufferSyntaxTreeSlot`]。
    /// desktop 渲染端 `slot.load()` 拿 `Arc<BufferSyntaxTree>` 跑 Query。
    pub fn tree_slot(&self) -> &BufferSyntaxTreeSlot {
        &self.tree_slot
    }

    pub fn language(&self) -> LanguageId {
        self.language
    }

    pub fn buffer_id(&self) -> BufferId {
        self.buffer_id
    }

    /// 编辑发生后调度入口。
    ///
    /// 主线程要做的事：
    /// 1. 同步把当前 slot 里 tree 的字节坐标推进到新版本（`tree.edit(InputEdit)`）。
    /// 这样 paint 端这一帧拿到的 tree 范围已经对齐新 snapshot —— 颜色不闪。
    /// 2. 投 `Job::Edit` 给 worker 去跑真正的增量 reparse。Worker 回来后通过 `store_if_newer` 把结构正确的 tree 覆盖 slot。
    ///
    /// 主线程消耗 = 一次 `Mutex` 临界区 + `Tree::clone`（`O(1)`）+ `tree.edit`（`O(log N)`） + 一次 channel send。
    /// **与文件大小无关**。
    pub fn handle_edit(&self, buffer: &Buffer, event: &DeltaEvent) {
        let new_snapshot = buffer.snapshot();

        if let Some(curr) = self.tree_slot.load() {
            if let Some(input_edits) =
                translate_edits(event.changeset(), curr.snapshot(), &new_snapshot)
            {
                self.tree_slot
                    .try_edit(&input_edits, new_snapshot.clone(), event.new_version());
            }
        }

        self.worker.edit(
            self.buffer_id,
            event.changeset().clone(),
            new_snapshot,
            event.new_version(),
        );
    }

    /// 关闭缓冲区 / 切换语言时清空 slot + 通知 worker。
    pub fn detach(self) {
        self.tree_slot.clear();
        self.worker.detach(self.buffer_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::SyntaxWorkerHandle;
    use crate::syntax::providers::rust::new_provider;
    use zom_engine::{Buffer, BufferConfig};

    fn make_buffer(text: &str) -> Buffer {
        Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
    }

    #[test]
    fn attach_populates_tree_slot_after_pump() {
        let buffer = make_buffer("fn main() {}");
        let worker = Arc::new(SyntaxWorkerHandle::spawn());
        let syntax = BufferSyntax::attach(
            BufferId::from_raw(1),
            LanguageId::new("rust"),
            Box::new(new_provider()),
            &buffer,
            worker.clone(),
        );
        worker.wait_for_idle_for_test_or_bench();

        let tree = syntax
            .tree_slot()
            .load()
            .expect("attach 完成后 slot 必须有 tree");
        assert_eq!(tree.version(), buffer.version());
        assert_eq!(syntax.language(), LanguageId::new("rust"));
    }

    #[test]
    fn detach_clears_tree_slot() {
        let buffer = make_buffer("fn main() {}");
        let worker = Arc::new(SyntaxWorkerHandle::spawn());
        let syntax = BufferSyntax::attach(
            BufferId::from_raw(2),
            LanguageId::new("rust"),
            Box::new(new_provider()),
            &buffer,
            worker.clone(),
        );
        worker.wait_for_idle_for_test_or_bench();
        assert!(syntax.tree_slot().load().is_some());

        let slot = syntax.tree_slot().clone();
        syntax.detach();
        worker.wait_for_idle_for_test_or_bench();
        assert!(
            slot.load().is_none(),
            "detach 后 slot 必须为空（不让下一任 provider 读到旧 tree）"
        );
    }
}
