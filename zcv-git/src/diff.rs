//! 行级差异的变化类型。
//!
//! 行级 diff 的解析与定位已归多缓冲区（从 base/working 快照直接派生），
//! 本 crate 只保留编辑器各处共用的变化类型判定。

/// hunk 变化类型（判定规则：旧侧空→Added、新侧空→Deleted）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiffHunkKind {
    /// 旧侧计数为 0（纯新增）。
    Added,
    /// 新旧两侧计数均非 0。
    Modified,
    /// 新侧计数为 0（纯删除）。
    Deleted,
}
