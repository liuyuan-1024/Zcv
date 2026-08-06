//! 行级 diff 数据模型（对齐 Zed 的 `buffer_diff` crate 归属）。
//!
//! 类型归属层：`zcv-git`（解析 git diff 输出）与 `zcv-editor`（渲染注入）共用，避免消费方各自定义同构类型再做转换。

use std::ops::Range;

/// 单块行级 diff：新侧逻辑行范围（0-based，左闭右开）+ 旧侧范围 + 变化类型。
///
/// - Added/Modified：range 为新增行的行号区间（纯增时旧侧计数为 0）；
/// - Deleted：range 为空区间，锚定 newStart−1 行（删除发生处的行），渲染侧展开为一个显示行。
/// - `old_range` 是旧侧（HEAD 版本）的行范围：Deleted 时用它从 HEAD 文本切片出被删除的行。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffHunk {
    pub range: Range<usize>,
    /// 旧侧（HEAD）行范围：Deleted 为被删除行；Added 为 oldStart..oldStart；Modified 两侧同行。
    pub old_range: Range<usize>,
    pub kind: DiffHunkKind,
}

/// hunk 变化类型（判定规则对齐 Zed buffer_diff：旧侧空→Added、新侧空→Deleted）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffHunkKind {
    /// 旧侧计数为 0（纯新增）。
    Added,
    /// 新旧两侧计数均非 0。
    Modified,
    /// 新侧计数为 0（纯删除）。
    Deleted,
}
