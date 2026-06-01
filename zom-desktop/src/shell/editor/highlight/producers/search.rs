//! search producer——把 BufferSearch 命中翻译为 Background Decoration。
//!
//! BufferSearch 由 `zom-workspace` 维护、per-buffer 共享。本函数只读快照；
//! 重跑 / try_remap 的责任在 panel 输入流 / 编辑流（见 `app.rs`）。current hit
//! 用 [`StyleClass::SearchCurrentBackground`] + 高一档优先级，让暖黄盖在普通
//! 命中之上、与选区区分开。

use zom_workspace::WorkspaceBuffer;

use crate::shell::editor::highlight::{
    Decoration, DecorationKind, DecorationStyle, StyleClass, priority,
};

pub(crate) fn push(buffer: &WorkspaceBuffer, out: &mut Vec<Decoration>) {
    let search = buffer.search();
    let current = search.current_range();
    for hit in search.ranges() {
        let (style, prio) = if Some(hit) == current {
            (
                DecorationStyle::Named(StyleClass::SearchCurrentBackground),
                priority::SEARCH_CURRENT,
            )
        } else {
            (
                DecorationStyle::Named(StyleClass::SearchNormalBackground),
                priority::SEARCH_NORMAL,
            )
        };
        out.push(Decoration {
            range: hit,
            kind: DecorationKind::Background,
            style,
            priority: prio,
        });
    }
}
