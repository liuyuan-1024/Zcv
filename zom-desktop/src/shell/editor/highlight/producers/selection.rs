//! selection producer——把非空 selection 翻译为 Background Decoration。
//!
//! caret-only selection 不产装饰，其几何由 element 在阶段 5 自己从
//! [`EditorSnapshot::selection`](crate::shell::editor::EditorSnapshot::selection)
//! 取。主编辑区与所有单行输入框都走本路径（见 [`build_snapshot`] 内的调用）。
//!
//! [`build_snapshot`]: crate::shell::editor::build_snapshot

use zom_engine::SelectionSet;

use crate::shell::editor::highlight::{
    Decoration, DecorationKind, DecorationStyle, StyleClass, priority,
};

pub(crate) fn push(selection: &SelectionSet, out: &mut Vec<Decoration>) {
    for sel in selection.as_slice().iter().filter(|s| !s.is_caret()) {
        out.push(Decoration {
            range: sel.range(),
            kind: DecorationKind::Background,
            style: DecorationStyle::Named(StyleClass::SelectionBackground),
            priority: priority::SELECTION,
        });
    }
}
