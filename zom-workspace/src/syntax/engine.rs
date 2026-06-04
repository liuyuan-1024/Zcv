//! `SyntaxEngine` —— 语法高亮子系统的共享资源池。
//!
//! 把原本散在 [`crate::Workspace`] 上的「语言注册表 + 后台 worker + buffer id 分配器」收口成一个值，
//! 按 [`std::rc::Rc`] 在主工作区与任意数量的嵌入式文档 ([`crate::SyntaxDocument`]) 之间共享：
//!
//! - **一次注册，全局可见**：组合根启动时通过 [`Self::registry_mut`] 注一遍内置 provider 工厂，所有持有同一 `Rc<SyntaxEngine>` 的容器都能 detect。
//! - **一根后台线程**：嵌入式编辑器不再各自 `SyntaxWorkerHandle::spawn`，都搭在共享 worker 上。
//! - **跨容器的稳定 buffer id**：worker 用 [`crate::BufferId`] 做任务寻址；id 由本结构集中分配，主工作区的常规缓冲区与嵌入文档不会撞 id。
//!
//! 启动期 [`SyntaxEngine::new`] 创建后**通常一次性配置完毕再 `Rc::new` 共享**
//! —— 注册表本身没用 RefCell，因为运行期只读路径足够。需要热插拔语言时再升级。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::BufferId;

use super::language::LanguageRegistry;
use super::worker::SyntaxWorkerHandle;

/// 语法高亮子系统的共享资源——语言注册表、后台 worker、buffer id 分配器。
///
/// 不可 `Clone`：跨容器共享通过 `Rc<SyntaxEngine>` 完成。
/// `SyntaxWorkerHandle` 内部已是 `Arc`，跨 `Rc<SyntaxEngine>` 引用同一根后台线程。
#[derive(Debug)]
pub struct SyntaxEngine {
    registry: LanguageRegistry,
    worker: Arc<SyntaxWorkerHandle>,
    next_buffer_id: AtomicU64,
}

impl SyntaxEngine {
    /// 全新引擎：空注册表 + 新启动的 worker 线程 + id 起点 1。
    pub fn new() -> Self {
        Self {
            registry: LanguageRegistry::new(),
            worker: Arc::new(SyntaxWorkerHandle::spawn()),
            next_buffer_id: AtomicU64::new(1),
        }
    }

    pub fn registry(&self) -> &LanguageRegistry {
        &self.registry
    }

    /// 仅启动期可变：组合根在 `Rc::new` 之前注册内置 provider 工厂。
    pub fn registry_mut(&mut self) -> &mut LanguageRegistry {
        &mut self.registry
    }

    pub fn worker(&self) -> &Arc<SyntaxWorkerHandle> {
        &self.worker
    }

    /// 分配下一个全局唯一 [`BufferId`]。
    ///
    /// 主工作区的 `open_*` 与 [`crate::SyntaxDocument::new`] 都走这条入口，
    /// 保证共享 worker 上的任务寻址不会撞 id。
    pub fn allocate_buffer_id(&self) -> BufferId {
        BufferId::from_raw(self.next_buffer_id.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for SyntaxEngine {
    fn default() -> Self {
        Self::new()
    }
}
