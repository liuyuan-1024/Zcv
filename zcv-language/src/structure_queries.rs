//! 结构查询：括号配对、大纲、缩进、折叠、文本对象。
//!
//! 各查询在同一模式的路径上执行：取与范围相交的语法层 → 在每层跑 tree-sitter 查询 → 收集结果并排序。

use std::ops::Range;
use std::sync::Arc;

use tree_sitter::StreamingIterator;
use zcv_engine::{ByteOffset, Line, Snapshot};

use crate::syntax_map::SyntaxSnapshot;
use crate::tree_sitter_utils::{QueryCursorHandle, SnapshotTextProvider};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BracketPair {
    pub open: Range<usize>,
    pub close: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutlineItem {
    pub range: Range<usize>,
    pub name_ranges: Vec<Range<usize>>,
    pub context_ranges: Vec<Range<usize>>,
    pub body_range: Option<Range<usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndentRange {
    pub range: Range<usize>,
    pub end: Option<Range<usize>>,
}

/// 一个可折叠范围。
///
/// 起点行在折叠后保留（折叠箭头显示在该行），范围内其余行隐藏。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldRange {
    pub range: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextObjectRange {
    pub kind: Arc<str>,
    pub range: Range<usize>,
}

impl SyntaxSnapshot {
    pub fn bracket_pairs(&self, range: Range<usize>, text: &Snapshot) -> Vec<BracketPair> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }
        let mut pairs = Vec::new();
        for layer in self.layers_for_range(&range) {
            let Some(query) = layer.language.brackets() else {
                continue;
            };
            let names = query.capture_names();
            let mut cursor = QueryCursorHandle::new();
            cursor.set_byte_range(range.clone());
            let mut matches =
                cursor.matches(query, layer.tree.root_node(), SnapshotTextProvider(text));
            while let Some(query_match) = matches.next() {
                let mut open = None;
                let mut close = None;
                for capture in query_match.captures {
                    match names.get(capture.index as usize).copied() {
                        Some("open") => open = Some(capture.node.byte_range()),
                        Some("close") => close = Some(capture.node.byte_range()),
                        _ => {}
                    }
                }
                if let (Some(open), Some(close)) = (open, close) {
                    pairs.push(BracketPair { open, close });
                }
            }
        }
        pairs.sort_unstable_by_key(|pair| (pair.open.start, pair.close.end));
        pairs
    }

    pub fn outline_items(&self, range: Range<usize>, text: &Snapshot) -> Vec<OutlineItem> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }
        let mut items = Vec::new();
        for layer in self.layers_for_range(&range) {
            let Some(query) = layer.language.outline() else {
                continue;
            };
            let names = query.capture_names();
            let mut cursor = QueryCursorHandle::new();
            cursor.set_byte_range(range.clone());
            let mut matches =
                cursor.matches(query, layer.tree.root_node(), SnapshotTextProvider(text));
            while let Some(query_match) = matches.next() {
                let mut item = None;
                let mut names_ranges = Vec::new();
                let mut contexts = Vec::new();
                let mut open = None;
                let mut close = None;
                for capture in query_match.captures {
                    let capture_range = capture.node.byte_range();
                    match names.get(capture.index as usize).copied() {
                        Some("item") => item = Some(capture_range),
                        Some("name") => names_ranges.push(capture_range),
                        Some("context") => contexts.push(capture_range),
                        Some("open") => open = Some(capture_range.end),
                        Some("close") => close = Some(capture_range.start),
                        _ => {}
                    }
                }
                let Some(item) = item else { continue };
                items.push(OutlineItem {
                    range: item,
                    name_ranges: names_ranges,
                    context_ranges: contexts,
                    body_range: open
                        .zip(close)
                        .and_then(|(start, end)| (start <= end).then_some(start..end)),
                });
            }
        }
        items.sort_unstable_by_key(|item| (item.range.start, item.range.end));
        items
    }

    pub fn indent_ranges(&self, range: Range<usize>, text: &Snapshot) -> Vec<IndentRange> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }
        let mut ranges = Vec::new();
        for layer in self.layers_for_range(&range) {
            let Some(query) = layer.language.indents() else {
                continue;
            };
            let names = query.capture_names();
            let mut cursor = QueryCursorHandle::new();
            cursor.set_byte_range(range.clone());
            let mut matches =
                cursor.matches(query, layer.tree.root_node(), SnapshotTextProvider(text));
            while let Some(query_match) = matches.next() {
                let mut indent = None;
                let mut end = None;
                for capture in query_match.captures {
                    match names.get(capture.index as usize).copied() {
                        Some("indent") => indent = Some(capture.node.byte_range()),
                        Some("end") => end = Some(capture.node.byte_range()),
                        _ => {}
                    }
                }
                if let Some(range) = indent {
                    ranges.push(IndentRange { range, end });
                }
            }
        }
        ranges.sort_unstable_by_key(|range| (range.range.start, range.range.end));
        ranges
    }

    pub fn fold_ranges(&self, range: Range<usize>, text: &Snapshot) -> Vec<FoldRange> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }
        let mut ranges = Vec::new();
        for layer in self.layers_for_range(&range) {
            let Some(query) = layer.language.folds() else {
                continue;
            };
            let names = query.capture_names();
            let mut cursor = QueryCursorHandle::new();
            cursor.set_byte_range(range.clone());
            let mut matches =
                cursor.matches(query, layer.tree.root_node(), SnapshotTextProvider(text));
            while let Some(query_match) = matches.next() {
                // 同一个 match 命中多个节点时（如 `+` 组捕获的连续注释），行相邻则合并成一个折叠范围。
                let mut nodes: Vec<_> = query_match
                    .captures
                    .iter()
                    .filter(|capture| {
                        names
                            .get(capture.index as usize)
                            .is_some_and(|name| &**name == "fold")
                    })
                    .map(|capture| capture.node)
                    .collect();
                nodes.sort_unstable_by_key(|node| node.byte_range().start);
                let mut merged: Vec<(Range<usize>, usize, usize)> = Vec::new();
                for node in nodes {
                    let byte_range = node.byte_range();
                    match merged.last_mut() {
                        Some((range, _, end_row)) if node.start_position().row <= *end_row + 1 => {
                            range.end = range.end.max(byte_range.end);
                            *end_row = node.end_position().row;
                        }
                        _ => {
                            merged.push((
                                byte_range,
                                node.start_position().row,
                                node.end_position().row,
                            ));
                        }
                    }
                }
                for (byte_range, _, _) in merged {
                    // 单行范围没有可隐藏的行，折叠无意义。
                    //
                    // 行判断用 buffer 行语义：line_comment 等节点含尾随换行（end 落在下一行行首），按"结束恰在行首则回退一行"换算真实末行。
                    let Ok(start_line) = text.byte_to_line(ByteOffset::new(byte_range.start))
                    else {
                        continue;
                    };
                    let mut end_line = text
                        .byte_to_line(ByteOffset::new(byte_range.end))
                        .unwrap_or(start_line);
                    if end_line > start_line
                        && text
                            .line_start_byte(end_line)
                            .is_ok_and(|start| start.get() == byte_range.end)
                    {
                        end_line = Line::new(end_line.get() - 1);
                    }
                    if start_line == end_line {
                        continue;
                    }
                    ranges.push(FoldRange { range: byte_range });
                }
            }
        }
        ranges.sort_unstable_by_key(|range| (range.range.start, range.range.end));
        ranges
    }

    pub fn text_object_ranges(&self, range: Range<usize>, text: &Snapshot) -> Vec<TextObjectRange> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }
        let mut ranges = Vec::new();
        for layer in self.layers_for_range(&range) {
            let Some(query) = layer.language.text_objects() else {
                continue;
            };
            let names = query.capture_names();
            let mut cursor = QueryCursorHandle::new();
            cursor.set_byte_range(range.clone());
            let mut captures =
                cursor.captures(query, layer.tree.root_node(), SnapshotTextProvider(text));
            while let Some((query_match, capture_index)) = captures.next() {
                let capture = query_match.captures[*capture_index];
                let Some(kind) = names.get(capture.index as usize) else {
                    continue;
                };
                ranges.push(TextObjectRange {
                    kind: Arc::from(*kind),
                    range: capture.node.byte_range(),
                });
            }
        }
        ranges.sort_unstable_by_key(|range| (range.range.start, range.range.end));
        ranges
    }
}

#[cfg(test)]
mod tests {
    use crate::syntax_map::rust_buffer;

    #[test]
    fn rust_syntax_snapshot_exposes_zed_structure_queries() {
        let source = "struct Demo { value: i32 }\nfn main() { let x = (1 + 2); }\n";
        let (buffer, syntax) = rust_buffer(source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let full = 0..snapshot.len_bytes().get();

        let brackets = syntax.bracket_pairs(full.clone(), &snapshot);
        assert!(brackets.iter().any(|pair| {
            &source[pair.open.clone()] == "(" && &source[pair.close.clone()] == ")"
        }));

        let outline = syntax.outline_items(full.clone(), &snapshot);
        let names: Vec<_> = outline
            .iter()
            .flat_map(|item| item.name_ranges.iter())
            .map(|range| &source[range.clone()])
            .collect();
        assert!(names.contains(&"Demo"));
        assert!(names.contains(&"main"));
        assert!(outline.iter().any(|item| item.body_range.is_some()));

        assert!(!syntax.indent_ranges(full.clone(), &snapshot).is_empty());
        assert!(
            syntax
                .text_object_ranges(full, &snapshot)
                .iter()
                .any(|range| range.kind.as_ref() == "function.around")
        );
    }

    #[test]
    fn rust_fold_ranges_cover_blocks_and_skip_single_lines() {
        let source = "\
struct Demo {
    value: i32,
}

impl Demo {
    fn new() -> Self {
        // 单行注释不产生折叠。
        let value = 1;
        // 连续注释折叠为一个组。
        // 第二行注释。
        Self { value }
    }
}

fn main() {
    let x = 1;
}
";
        let (buffer, syntax) = rust_buffer(source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let folds = syntax.fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
        let texts: Vec<&str> = folds
            .iter()
            .map(|fold| &source[fold.range.clone()])
            .collect();

        // 各块体（field_declaration_list / declaration_list / block）都被覆盖。
        assert!(texts.contains(&"{\n    value: i32,\n}"));
        assert!(texts.contains(&"{\n    fn new() -> Self {\n        // 单行注释不产生折叠。\n        let value = 1;\n        // 连续注释折叠为一个组。\n        // 第二行注释。\n        Self { value }\n    }\n}"));
        assert!(texts.contains(&"{\n        // 单行注释不产生折叠。\n        let value = 1;\n        // 连续注释折叠为一个组。\n        // 第二行注释。\n        Self { value }\n    }"));
        // 连续注释组折叠为一个范围。
        assert!(texts.contains(&"// 连续注释折叠为一个组。\n        // 第二行注释。"));

        // 嵌套结构：外层范围完整包含内层范围。
        let outer = folds
            .iter()
            .find(|fold| &source[fold.range.clone()] == "{\n    fn new() -> Self {\n        // 单行注释不产生折叠。\n        let value = 1;\n        // 连续注释折叠为一个组。\n        // 第二行注释。\n        Self { value }\n    }\n}")
            .unwrap();
        let inner = folds
            .iter()
            .find(|fold| {
                fold.range.start >= outer.range.start
                    && fold.range.end <= outer.range.end
                    && fold.range != outer.range
            })
            .expect("impl 块内应存在嵌套折叠范围");

        assert!(inner.range.start > outer.range.start && inner.range.end < outer.range.end);
    }

    #[test]
    fn use_declarations_fold_independently_and_skip_single_lines() {
        let source = "\
use std::collections::BTreeMap;
use std::ops::{
    Range,
    Deref,
};
use std::sync::Arc;

use zcv_engine::{
    Buffer,
    Snapshot,
};
";
        let (buffer, syntax) = rust_buffer(source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let folds = syntax.fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
        let texts: Vec<&str> = folds
            .iter()
            .map(|fold| &source[fold.range.clone()])
            .collect();

        // 两个多行 use 各自独立成折叠范围（覆盖整个声明，不与相邻 use 合并）。
        assert!(texts.contains(&"use std::ops::{\n    Range,\n    Deref,\n};"));
        assert!(texts.contains(&"use zcv_engine::{\n    Buffer,\n    Snapshot,\n};"));
        // 单行 use 不产生折叠。
        assert!(!texts.contains(&"use std::collections::BTreeMap;"));
    }

    #[test]
    fn single_line_doc_comments_do_not_fold() {
        // 回归：tree-sitter-rust 的 line_comment 节点含尾随换行（end 落在下一行行首），
        // 单行过滤必须用 buffer 行语义，否则单行注释会误判为可折叠。
        let source = "\
/// Editor 自身的领域事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorEvent {
    /// 编辑器关联的文件路径发生变化。
    PathChanged,
}
";
        let (buffer, syntax) = rust_buffer(source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let folds = syntax.fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
        let texts: Vec<&str> = folds
            .iter()
            .map(|fold| &source[fold.range.clone()])
            .collect();

        // 只有 enum 块体可折叠；单行 doc 注释不产生折叠。
        assert!(texts.contains(&"{\n    /// 编辑器关联的文件路径发生变化。\n    PathChanged,\n}"));
        assert!(!texts.iter().any(|text| text.starts_with("///")));
    }

    #[test]
    fn multi_line_macro_invocation_folds_but_single_line_does_not() {
        let source = "\
fn main() {
    let x = vec![
        1,
        2,
    ];
    println!(\"ok\");
    actions!(
        editor,
        [
            MoveLeft,
            MoveRight,
        ],
    );
    let y = format!(\"{}: {}\", 1, 2);
}
";
        let (buffer, syntax) = rust_buffer(source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let folds = syntax.fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
        let texts: Vec<&str> = folds
            .iter()
            .map(|fold| &source[fold.range.clone()])
            .collect();

        // 跨行宏调用整体成折叠范围（从宏名到分号）。
        assert!(texts.contains(&"vec![\n        1,\n        2,\n    ]"));
        assert!(texts.contains(&"actions!(\n        editor,\n        [\n            MoveLeft,\n            MoveRight,\n        ],\n    )"));
        // 单行宏调用不产生折叠。
        assert!(!texts.contains(&"println!(\"ok\")"));
        assert!(!texts.contains(&"format!(\"{}: {}\", 1, 2)"));
    }
}
