//! syntax producer —— 把当前 [`BufferSyntaxTree`] 在 viewport 上的 query 结果翻译为 Foreground Decoration。
//!
//! 计划 §Phase 2：颜色 = 当前 tree 在 viewport 上的纯函数。
//! 没有缓存、没有 layer、没有"先看到旧颜色再看到新颜色"的中间帧——每帧从共享的 [`BufferSyntaxTree`] 出发跑一次 viewport-scoped tree-sitter Query。
//!
//! `highlight name → 字色` 的解析归 shell composer；producer 只把语法高亮名保留在 [`StyleClass::Syntax`] 里，避免应用域知道主题实现。
//!
//! ## 为什么 thread-local 缓存 cursor
//!
//! `tree_sitter::QueryCursor` 构造一次还行，每帧若干次会累积分配。把它放在线程局部槽位里跨帧复用，把每帧的 cursor 构造代价摊掉。
//! `SyntaxQueryCursor` 是 zom-workspace 暴露的薄 newtype —— desktop 不必直接依赖 tree-sitter。

use std::cell::RefCell;

use zom_engine::{ByteOffset, TextRange};
use zom_workspace::syntax::{BufferSyntaxTree, SyntaxQueryCursor};

use crate::editor::highlight::{Decoration, DecorationKind, DecorationStyle, StyleClass, priority};
use crate::editor::text::snapshot::SnapshotLine;

thread_local! {
    /// 渲染线程跨帧复用的 query cursor。
    /// RefCell 允许 push 内部 borrow_mut，同线程不会重入 paint，本不会撞 borrow。
    static QUERY_CURSOR: RefCell<SyntaxQueryCursor> = RefCell::new(SyntaxQueryCursor::new());
}

pub(crate) fn push(
    syntax_tree: Option<&BufferSyntaxTree>,
    snapshot_lines: &[SnapshotLine],
    out: &mut Vec<Decoration>,
) {
    let Some(st) = syntax_tree else { return };
    let Some(viewport) = viewport_byte_range(snapshot_lines) else {
        return;
    };
    let spans = QUERY_CURSOR.with(|cell| {
        let mut cursor = cell.borrow_mut();
        st.query_viewport(viewport, &mut cursor)
    });
    for (range, span) in spans {
        out.push(Decoration {
            range,
            kind: DecorationKind::Foreground,
            style: DecorationStyle::Named(StyleClass::Syntax(span.name.to_string())),
            priority: priority::SYNTAX_CONFIRMED,
        });
    }
}

fn viewport_byte_range(lines: &[SnapshotLine]) -> Option<TextRange> {
    let first = lines.first()?;
    let last = lines.last()?;
    let start = ByteOffset::new(first.start_byte);
    let end = ByteOffset::new(last.start_byte + last.text.len());
    TextRange::new(start, end).ok()
}
