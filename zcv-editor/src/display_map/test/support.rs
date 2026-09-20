//! 显示层测试共享的只读访问器。
//!
//! 显示热路径按显示行消费无行终止符 chunk；这里保留测试对投影行原文的断言入口。

use std::borrow::Cow;

use zcv_text::Line;

pub(crate) use super::super::wrap_map::WrapRowKind;

use super::super::{DisplaySnapshot, ProjectedLineIndex};

/// 单个投影行的完整文本（折叠合并行含占位符，保留尾部换行）。
pub(crate) fn projected_line_text(
    snapshot: &DisplaySnapshot,
    projected_line: usize,
) -> Option<Cow<'_, str>> {
    let row = ProjectedLineIndex::new(projected_line);
    let fold = snapshot.fold_snapshot();
    if fold.is_fold_row(row) {
        fold.row_text(row)
    } else {
        snapshot
            .wrap_snapshot()
            .tab_snapshot()
            .line_text(Line::new(projected_line))
    }
}
