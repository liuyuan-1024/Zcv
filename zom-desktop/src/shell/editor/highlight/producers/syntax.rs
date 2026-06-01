//! syntax producer——把 buffer 的 `syntax` MetadataLayer 与视口相交的 span 翻
//! 译为 Foreground Decoration。
//!
//! `highlight name → 字色` 在 producer 端按 theme 解析（详见
//! [`crate::shell::editor::highlight`] 模块顶部的 `Resolved` 说明，与高亮架构
//! 手册 §七）。落 Decoration 时用 [`DecorationStyle::Resolved`]，composer 不再
//! 二次查表。
//!
//! 视口字节区间从 `snapshot_lines` 现算——避免与调用方持有的
//! `&mut snapshot.decorations` 借用冲突。

use zom_engine::{ByteOffset, TextRange};
use zom_workspace::WorkspaceBuffer;
use zom_workspace::syntax::syntax_layer_kind;

use crate::shell::editor::highlight::{Decoration, DecorationKind, DecorationStyle, priority};
use crate::shell::editor::snapshot::SnapshotLine;
use crate::shell::shared::theme::syntax;

pub(crate) fn push(
    buffer: &WorkspaceBuffer,
    snapshot_lines: &[SnapshotLine],
    out: &mut Vec<Decoration>,
) {
    let Some(query_range) = viewport_byte_range(snapshot_lines) else {
        return;
    };
    let kind = syntax_layer_kind();
    for entry in buffer
        .highlight_layers()
        .ranges_for_kind_intersecting(&kind, query_range)
    {
        let color = syntax::color_for(entry.metadata().name.as_str());
        out.push(Decoration {
            range: entry.range(),
            kind: DecorationKind::Foreground,
            style: DecorationStyle::Resolved(color),
            priority: priority::SYNTAX_BASE,
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
