//! 显示投影层的坐标编辑原语。
//!
//! 等价于 Zed 的 `text::Edit<D>`：每一层在 `sync(lower_snapshot, lower_edits)` 中消费下层坐标空间的编辑，并发布自己坐标空间中的编辑。
//! `old` 落在旧快照坐标系，`new` 落在新快照坐标系。

use std::ops::{Add, Range, Sub};

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

impl<D> ProjectionEdit<D>
where
    D: Copy + Ord + Sub<D, Output = usize>,
{
    pub(crate) fn old_len(&self) -> usize {
        self.old.end - self.old.start
    }

    pub(crate) fn new_len(&self) -> usize {
        self.new.end - self.new.start
    }
}

/// 把单个投影坐标沿有序、互不相交的编辑列表推进到新坐标空间。
///
/// 折叠端点与 inlay 位置都由此从旧坐标得到新坐标，不再依赖文本层的 PositionMap。
/// 落在编辑区间内的坐标吸附到新区间内对应位置；编辑区间之后的坐标整体平移。
pub(crate) fn remap_offset<D>(offset: D, edits: &[ProjectionEdit<D>]) -> Option<D>
where
    D: Copy + Ord + Default + Add<usize, Output = D> + Sub<D, Output = usize>,
{
    let mut delta: isize = 0;
    for edit in edits {
        if offset < edit.old.start {
            break;
        }
        if offset < edit.old.end {
            let within = offset - edit.old.start;
            return Some(edit.new.start + within.min(edit.new_len()));
        }
        delta += edit.new_len() as isize - edit.old_len() as isize;
    }
    let relative = offset - D::default();
    let mapped = relative as isize + delta;
    (mapped >= 0).then(|| D::default() + mapped as usize)
}
