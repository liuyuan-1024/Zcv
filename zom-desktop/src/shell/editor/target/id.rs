//! 编辑器输入目标：系统输入法、编辑命令与嵌入控件之间的稳定身份。
//!
//! 编辑器子系统不认识具体的调用方 —— `TextTargetId` 是个不透明值，由宿主在
//! 装配时通过 [`TextTargetId::allocate`] 分配，路由器只用它做 `==` 比较。
//! 任何在 `shell/editor/**` 之外的"按 id 走分支"都是调用方自己的事。

use std::sync::atomic::{AtomicU64, Ordering};

/// 一个可接收文本输入的编辑目标的不透明身份。
///
/// 同一进程内全局唯一；进程间不持久。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct TextTargetId(u64);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl TextTargetId {
    /// 分配一个新 id。线程安全。
    pub(crate) fn allocate() -> Self {
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// 内部数值 —— 仅供生成稳定 element id / 调试输出使用，不要在业务侧
    /// 据此 `match`。
    pub(crate) fn raw(self) -> u64 {
        self.0
    }
}

/// 应用层维护的 5 个嵌入式文本目标 id 注册表。
///
/// 编辑器子系统不知道有哪些调用方；这只是宿主把"我家有这几个目标"集中持
/// 一份，方便 [`crate::app::App`] / [`crate::shell::view`] 装配时把 id 派发
/// 给各 feature 模型与 slot。新增嵌入点时在这里加一条字段。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TextTargetIds {
    pub(crate) main_editor: TextTargetId,
    pub(crate) file_tree_pending_name: TextTargetId,
    pub(crate) project_picker_query: TextTargetId,
    pub(crate) search_query: TextTargetId,
    pub(crate) search_replacement: TextTargetId,
}

impl TextTargetIds {
    pub(crate) fn allocate() -> Self {
        Self {
            main_editor: TextTargetId::allocate(),
            file_tree_pending_name: TextTargetId::allocate(),
            project_picker_query: TextTargetId::allocate(),
            search_query: TextTargetId::allocate(),
            search_replacement: TextTargetId::allocate(),
        }
    }
}
