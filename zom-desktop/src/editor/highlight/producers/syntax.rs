//! syntax producer——把任意「挂着语法高亮的 buffer」的 layer 与视口相交的
//! span 翻译为 Foreground Decoration。
//!
//! `highlight name → 字色` 的解析归 shell composer；producer 只把语法高亮名
//! 保留在 [`StyleClass::Syntax`] 里，避免应用域知道主题实现。
//!
//! 入参是裸的 [`MetadataLayers<HighlightSpan>`]——既兼容主工作区的
//! [`zom_workspace::WorkspaceBuffer`]，又兼容嵌入式的
//! [`zom_workspace::SyntaxDocument`]。视口字节区间从 `snapshot_lines` 现算，
//! 避免与调用方持有的 `&mut snapshot.decorations` 借用冲突。

use zom_engine::{ByteOffset, MetadataLayers, TextRange};
use zom_workspace::syntax::{
    HighlightSpan, syntax_confirmed_layer_kind, syntax_provisional_layer_kind,
};

use crate::editor::highlight::{Decoration, DecorationKind, DecorationStyle, StyleClass, priority};
use crate::editor::text::snapshot::SnapshotLine;

pub(crate) fn push(
    layers: &MetadataLayers<HighlightSpan>,
    snapshot_lines: &[SnapshotLine],
    out: &mut Vec<Decoration>,
) {
    let Some(query_range) = viewport_byte_range(snapshot_lines) else {
        return;
    };
    push_layer(
        layers,
        syntax_confirmed_layer_kind(),
        query_range,
        priority::SYNTAX_CONFIRMED,
        out,
    );
    push_layer(
        layers,
        syntax_provisional_layer_kind(),
        query_range,
        priority::SYNTAX_PROVISIONAL,
        out,
    );
}

fn push_layer(
    layers: &MetadataLayers<HighlightSpan>,
    kind: zom_engine::MetadataLayerKind,
    query_range: TextRange,
    priority: u16,
    out: &mut Vec<Decoration>,
) {
    for entry in layers.ranges_for_kind_intersecting(&kind, query_range) {
        out.push(Decoration {
            range: entry.range(),
            kind: DecorationKind::Foreground,
            style: DecorationStyle::Named(StyleClass::Syntax(entry.metadata().name.to_string())),
            priority,
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

#[cfg(test)]
mod tests {
    use super::*;
    use zom_workspace::syntax::HighlightName;

    fn range(start: usize, end: usize) -> TextRange {
        TextRange::new(ByteOffset::new(start), ByteOffset::new(end)).unwrap()
    }

    fn span(name: &'static str) -> HighlightSpan {
        HighlightSpan::from_name(HighlightName::new(name))
    }

    #[test]
    fn push_emits_confirmed_and_provisional_with_ordered_priorities() {
        let mut layers = MetadataLayers::new();
        layers
            .replace_layer_ranges(
                syntax_confirmed_layer_kind(),
                zom_engine::BufferVersion::INITIAL,
                vec![(range(0, 6), span("keyword"))],
            )
            .unwrap();
        layers
            .replace_layer_ranges(
                syntax_provisional_layer_kind(),
                zom_engine::BufferVersion::INITIAL,
                vec![(range(2, 4), span("string"))],
            )
            .unwrap();

        let lines = vec![SnapshotLine {
            line_index: 0,
            start_byte: 0,
            text: "abcdef".to_string(),
        }];
        let mut out = Vec::new();
        push(&layers, &lines, &mut out);

        assert_eq!(out.len(), 2);
        assert!(
            out.iter()
                .any(|d| { d.priority == priority::SYNTAX_CONFIRMED && d.range == range(0, 6) }),
            "confirmed decoration 应使用 confirmed priority，实际 {out:?}",
        );
        assert!(
            out.iter()
                .any(|d| { d.priority == priority::SYNTAX_PROVISIONAL && d.range == range(2, 4) }),
            "provisional decoration 应使用更高 priority，实际 {out:?}",
        );
    }
}
