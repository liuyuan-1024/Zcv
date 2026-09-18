//! 显示投影层的坐标编辑原语。
//!
//! 等价于 Zed 的 `text::Edit<D>`：每一层在 `sync(lower_snapshot, lower_edits)` 中消费下层坐标空间的编辑，并发布自己坐标空间中的编辑。
//! `old` 落在旧快照坐标系，`new` 落在新快照坐标系。

use std::ops::Range;

/// 一段投影变化在旧、新坐标空间的覆盖范围。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProjectionEdit<D> {
    pub(crate) old: Range<D>,
    pub(crate) new: Range<D>,
}

impl<D> ProjectionEdit<D> {
    pub(crate) fn new(old: Range<D>, new: Range<D>) -> Self {
        Self { old, new }
    }
}
