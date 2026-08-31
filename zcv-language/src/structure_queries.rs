//! 结构查询：括号配对、大纲、缩进、折叠、文本对象。
//!
//! 各查询在同一模式的路径上执行：取与范围相交的语法层 → 在每层跑 tree-sitter 查询 → 收集结果并排序。

use std::ops::Range;

use tree_sitter::StreamingIterator;
use zcv_text::{ByteOffset, Line, Snapshot, TextResult};

use crate::syntax_map::SyntaxSnapshot;
use crate::tree_sitter_utils::{QueryCursorHandle, SnapshotTextProvider};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BracketPair {
    pub open: Range<usize>,
    pub close: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndentRange {
    pub range: Range<usize>,
    pub end: Option<Range<usize>>,
}

/// 在指定光标位置按 Enter 后，目标行应采用的缩进。
///
/// `base_indent` 从最近的代码行继承，`additional_levels` 则由语言的 Tree-sitter 缩进查询决定。编辑器只需按其自身的 Tab 配置将额外层级转为空白字符。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewlineIndent {
    pub base_indent: String,
    pub additional_levels: usize,
}

/// 一个可折叠范围。
///
/// 范围 = [入口行行尾换行符, 闭合括号前)：入口行全文与闭合括号保留可见，折叠后占位符与闭合尾段拼在同一显示行。
/// 无括号对的折叠（注释组等）终点在末隐藏行内容末尾。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoldRange {
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

    /// 基于语言语法树计算在 `offset` 处换行时，下一行的建议缩进。
    ///
    /// 这与编辑器 UI 无关：语言层负责找到缩进基准和未闭合的语法结构，编辑器负责将结果应用到插入文本。
    pub fn suggested_newline_indent(
        &self,
        offset: ByteOffset,
        text: &Snapshot,
    ) -> TextResult<NewlineIndent> {
        let current_line = text.byte_to_line(offset)?;
        let line_start = text.line_start_byte(current_line)?;
        let prefix = text.slice_byte_range(line_start, offset)?;
        let (basis_line, base_indent) = newline_indent_basis(text, current_line, prefix.as_str())?;
        let query_start = offset.get().saturating_sub(1);
        let query_end = offset.get().saturating_add(1).min(text.len_bytes().get());
        let additional_levels = usize::from(
            self.indent_ranges(query_start..query_end, text)
                .into_iter()
                .any(|range| {
                    text.byte_to_line(ByteOffset::new(range.range.start)) == Ok(basis_line)
                        && range.range.start < offset.get()
                        && offset.get() < range.range.end
                        && range
                            .end
                            .as_ref()
                            .is_none_or(|end| offset.get() <= end.start)
                }),
        );
        Ok(NewlineIndent {
            base_indent,
            additional_levels,
        })
    }

    pub fn fold_ranges(&self, range: Range<usize>, text: &Snapshot) -> Vec<FoldRange> {
        if !self.can_query(&range, text) {
            return Vec::new();
        }
        // 折叠节点：@fold 捕获（块体等），行相邻合并（`(line_comment)+` 组）。
        let mut nodes = Vec::new();
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
                let mut captured: Vec<_> = query_match
                    .captures
                    .iter()
                    .filter(|capture| {
                        names
                            .get(capture.index as usize)
                            .is_some_and(|name| &**name == "fold")
                    })
                    .map(|capture| capture.node)
                    .collect();
                captured.sort_unstable_by_key(|node| node.byte_range().start);
                let mut merged: Vec<(Range<usize>, usize, usize)> = Vec::new();
                for node in captured {
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
                nodes.extend(merged);
            }
        }
        // 括号对：把折叠范围重塑为 [入口行行尾换行符, 闭合括号前)，闭合括号保留可见。
        let pairs = self.bracket_pairs(range.clone(), text);
        let mut ranges = Vec::new();
        for (byte_range, _, _) in nodes {
            let Ok(anchor_line) = text.byte_to_line(ByteOffset::new(byte_range.start)) else {
                continue;
            };
            let start = line_newline_position(text, anchor_line);
            let pair_index = pairs.partition_point(|pair| pair.open.start < byte_range.start);
            let enclosing_pair = pairs[pair_index..]
                .iter()
                .take_while(|pair| pair.open.start < byte_range.end)
                .filter(|pair| pair.close.end <= byte_range.end)
                .filter(|pair| {
                    text.byte_to_line(ByteOffset::new(pair.close.start))
                        .is_ok_and(|line| line > anchor_line)
                })
                .max_by_key(|pair| pair.close.end - pair.open.start);
            let end = enclosing_pair.map_or_else(
                || {
                    // 无括号对（注释组等）：整行兜底，终点 = 末隐藏行内容末尾。
                    //
                    // line_comment 等节点含尾随换行（end 落在下一行行首），按"结束恰在行首则回退一行"换算真实末行。
                    let mut end_line = text
                        .byte_to_line(ByteOffset::new(byte_range.end))
                        .unwrap_or(anchor_line);
                    if end_line > anchor_line
                        && text
                            .line_start_byte(end_line)
                            .is_ok_and(|start| start.get() == byte_range.end)
                    {
                        end_line = Line::new(end_line.get() - 1);
                    }
                    line_content_end(text, end_line)
                },
                |pair| ByteOffset::new(pair.close.start),
            );
            // 单行范围没有可隐藏的行，折叠无意义。
            if start >= end || text.byte_to_line(end).is_ok_and(|line| line <= anchor_line) {
                continue;
            }
            ranges.push(FoldRange {
                range: start.get()..end.get(),
            });
        }
        ranges.sort_unstable_by_key(|range| (range.range.start, range.range.end));
        ranges
    }
}

fn newline_indent_basis(
    text: &Snapshot,
    current_line: Line,
    prefix: &str,
) -> TextResult<(Line, String)> {
    if prefix
        .chars()
        .any(|character| !matches!(character, ' ' | '\t'))
    {
        return Ok((current_line, leading_whitespace(prefix)));
    }

    for line_index in (0..current_line.get()).rev() {
        let line = Line::new(line_index);
        let content = text.line_content(line, None)?;
        if content
            .as_str()
            .chars()
            .any(|character| !matches!(character, ' ' | '\t'))
        {
            return Ok((line, leading_whitespace(content.as_str())));
        }
    }

    Ok((current_line, leading_whitespace(prefix)))
}

fn leading_whitespace(text: &str) -> String {
    text.chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .collect()
}

/// 行终止换行符（`\n`）的字节位置；行尾无换行符时返回行尾。
///
/// 折叠范围从该位置开始：入口行换行符被折叠吞掉，占位符与闭合尾段拼在同一显示行。
fn line_newline_position(text: &Snapshot, line: Line) -> ByteOffset {
    let content = text
        .line_content(line, None)
        .expect("折叠入口行必须位于当前 Snapshot 内");
    if content.text_range().end() == content.full_range().end() {
        content.full_range().end()
    } else {
        // 行含终止换行符：`\r?\n` 的 `\n` 位于行尾前一字节。
        ByteOffset::new(content.full_range().end().get() - 1)
    }
}

/// 行内容末尾（不含终止换行符）。
fn line_content_end(text: &Snapshot, line: Line) -> ByteOffset {
    text.line_content(line, None)
        .expect("折叠末行必须位于当前 Snapshot 内")
        .text_range()
        .end()
}

#[cfg(test)]
mod tests {
    use super::NewlineIndent;
    use crate::syntax_map::{parsed_syntax, rust_buffer};
    use zcv_text::ByteOffset;

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

        assert!(!syntax.indent_ranges(full, &snapshot).is_empty());
    }

    #[test]
    fn baseline_languages_expose_brackets_indents_and_folds() {
        let cases = [
            ("main.c", "int main() {\n  return 0;\n}\n"),
            (
                "main.cpp",
                "class Greeter {\npublic:\n  void greet() {}\n};\n",
            ),
            (
                "Program.cs",
                "class Program {\n  static void Main() {}\n}\n",
            ),
            ("main.go", "package main\nfunc main() {\n  println(1)\n}\n"),
            (
                "app.rb",
                "class Greeter\n  def greet(name)\n    name\n  end\nend\n",
            ),
            (
                "index.php",
                "<?php\nfunction greet($name) {\n  return $name;\n}\n",
            ),
            (
                "main.swift",
                "struct Greeter {\n  func greet() {\n    print(1)\n  }\n}\n",
            ),
            (
                "Main.kt",
                "class Greeter {\n  fun greet() {\n    println(1)\n  }\n}\n",
            ),
            (
                "init.lua",
                "local function greet(name)\n  return name\nend\n",
            ),
            ("main.zig", "pub fn main() void {\n  const value = 1;\n}\n"),
            (
                "query.sql",
                "SELECT name\nFROM (\n  SELECT name FROM users\n) nested;\n",
            ),
        ];

        for (path, source) in cases {
            let (buffer, syntax) = parsed_syntax(path, source);
            let snapshot = buffer.snapshot();
            let syntax = syntax.snapshot();
            let full = 0..snapshot.len_bytes().get();
            assert!(
                !syntax.bracket_pairs(full.clone(), &snapshot).is_empty(),
                "{path} 应产生括号配对"
            );
            assert!(
                !syntax.indent_ranges(full.clone(), &snapshot).is_empty(),
                "{path} 应产生缩进范围"
            );
            assert!(
                !syntax.fold_ranges(full, &snapshot).is_empty(),
                "{path} 应产生折叠范围"
            );
        }
    }

    #[test]
    fn existing_languages_with_new_fold_queries_produce_ranges() {
        let cases = [
            ("main.py", "def greet():\n    return 1\n"),
            ("main.js", "function greet() {\n  return 1;\n}\n"),
            ("Main.java", "class Main {\n  static void main() {}\n}\n"),
            ("script.sh", "function greet() {\n  echo hi\n}\n"),
            ("Cargo.toml", "[package]\nname = \"zcv\"\nversion = \"1\"\n"),
            ("data.json", "{\n  \"name\": \"zcv\"\n}\n"),
            ("data.yaml", "root:\n  child:\n    value: 1\n"),
            ("README.md", "# 标题\n\n第一段。\n\n第二段。\n"),
            ("index.html", "<main>\n  <p>text</p>\n</main>\n"),
            ("style.css", ".card {\n  color: red;\n}\n"),
        ];

        for (path, source) in cases {
            let (buffer, syntax) = parsed_syntax(path, source);
            let snapshot = buffer.snapshot();
            let folds = syntax
                .snapshot()
                .fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
            assert!(!folds.is_empty(), "{path} 应产生折叠范围");
        }
    }

    #[test]
    fn rust_newline_indent_is_computed_in_the_language_layer() {
        let source = "fn main() {\n    build()\n}";
        let (buffer, syntax) = rust_buffer(source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let after_open_paren = source.find("build(").unwrap() + "build(".len();
        let after_closed_call = source.find("build()").unwrap() + "build()".len();

        assert_eq!(
            syntax
                .suggested_newline_indent(ByteOffset::new(after_open_paren), &snapshot)
                .unwrap(),
            NewlineIndent {
                base_indent: "    ".to_owned(),
                additional_levels: 1,
            }
        );
        assert_eq!(
            syntax
                .suggested_newline_indent(ByteOffset::new(after_closed_call), &snapshot)
                .unwrap(),
            NewlineIndent {
                base_indent: "    ".to_owned(),
                additional_levels: 0,
            }
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

        // 各块体（field_declaration_list / declaration_list / block）都被覆盖，
        // 范围 = [入口行换行符, 闭合括号前)：入口行与闭合括号不在范围内。
        assert!(texts.contains(&"\n    value: i32,\n"));
        assert!(texts.contains(&"\n    fn new() -> Self {\n        // 单行注释不产生折叠。\n        let value = 1;\n        // 连续注释折叠为一个组。\n        // 第二行注释。\n        Self { value }\n    }\n"));
        assert!(texts.contains(&"\n        // 单行注释不产生折叠。\n        let value = 1;\n        // 连续注释折叠为一个组。\n        // 第二行注释。\n        Self { value }\n    "));
        // 连续注释组折叠为一个范围（入口行保留，其余行隐藏）。
        assert!(texts.contains(&"\n        // 第二行注释。"));

        // 嵌套结构：外层范围完整包含内层范围。
        let outer = folds
            .iter()
            .find(|fold| {
                &source[fold.range.clone()]
                    == "\n    fn new() -> Self {\n        // 单行注释不产生折叠。\n        let value = 1;\n        // 连续注释折叠为一个组。\n        // 第二行注释。\n        Self { value }\n    }\n"
            })
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

use zcv_text::{
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

        // 两个多行 use 各自独立成折叠范围（入口行与闭合括号 `}` 保留，尾段 `;` 可见）。
        assert!(texts.contains(&"\n    Range,\n    Deref,\n"));
        assert!(texts.contains(&"\n    Buffer,\n    Snapshot,\n"));
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
        assert!(texts.contains(&"\n    /// 编辑器关联的文件路径发生变化。\n    PathChanged,\n"));
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

        // 跨行宏调用整体成折叠范围（入口行保留，闭合括号前收口：`vec![...]` 收在 `]` 前）。
        assert!(texts.contains(&"\n        1,\n        2,\n    "));
        assert!(texts.contains(&"\n        editor,\n        [\n            MoveLeft,\n            MoveRight,\n        ],\n    "));
        // 单行宏调用不产生折叠。
        assert!(!texts.contains(&"println!(\"ok\")"));
        assert!(!texts.contains(&"format!(\"{}: {}\", 1, 2)"));
    }
}
