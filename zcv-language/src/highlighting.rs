//! 语法高亮：按层 capture 流归并构建跨度。
//!
//! 高亮查询的结果是快照全局 capture 索引（跨主语言与注入语言唯一），渲染侧按索引查样式表，不再解析 capture 名。
//!
//! 每层（主层 + 注入层）单独产出按文档序排列的 capture 区间，然后 k 路归并 capture 起点；
//! 活动栈用区间终点恢复外层高亮，避免为结束位置额外构造一份全局事件数组。

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::ops::Range;

use tree_sitter::StreamingIterator;
use zcv_text::Snapshot;

use crate::Language;
use crate::syntax_map::SyntaxSnapshot;
use crate::tree_sitter_utils::{QueryCursorHandle, SnapshotTextProvider, ranges_overlap};

/// 一个非重叠的 tree-sitter capture 区间。
///
/// `capture` 是快照全局 capture 名字表的索引（跨主语言与注入语言唯一），渲染侧按索引查预展开的样式表，不再携带并逐 run 解析 capture 名。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub capture: u32,
}

/// 单层 capture 流中的一个区间。
#[derive(Clone, Copy, Debug)]
struct CaptureRange {
    start: usize,
    end: usize,
    capture: u32,
}

/// 归并队列项：起点相同时，外层区间和浅层语法树先入栈，内层高亮最后覆盖。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct QueueKey {
    start: usize,
    end: Reverse<usize>,
    depth: u32,
    seq: usize,
}

impl QueueKey {
    fn new(capture: &CaptureRange, depth: u32, seq: usize) -> Self {
        Self {
            start: capture.start,
            end: Reverse(capture.end),
            depth,
            seq,
        }
    }
}

impl SyntaxSnapshot {
    /// 查询指定字节范围，并让更内层、后出现的 capture 覆盖外层。
    ///
    /// 每层一个 capture 流（文档序），k 路归并后以全局活动栈直接产出 spans：
    /// 树中节点要么嵌套要么不相交，注入层 capture 又受其内容节点约束，因此全局栈的 LIFO 顺序就是覆盖顺序，栈顶即当前最内层。
    pub fn highlights(&self, range: Range<usize>, text: &Snapshot) -> Vec<HighlightSpan> {
        if text.version() != self.version || range.start >= range.end {
            return Vec::new();
        }
        let (Some(language), Some(tree)) = (&self.language, self.root_tree()) else {
            return Vec::new();
        };

        // 相关层（主层 + 与范围相交的注入层），每层产出有序 capture 流。
        let mut streams: Vec<(u32, Vec<CaptureRange>)> = Vec::new();
        if let Some(captures) = collect_capture_ranges(language, tree, &range, text, self) {
            streams.push((0, captures));
        }
        let mut injections: Vec<_> = self
            .injection_layers()
            .iter()
            .filter(|layer| ranges_overlap(&layer.range, &range))
            .map(|layer| (layer.depth, &layer.language, &layer.tree))
            .collect();
        injections.sort_unstable_by_key(|(depth, _, _)| *depth);
        for (depth, language, tree) in injections {
            if let Some(captures) = collect_capture_ranges(language, tree, &range, text, self) {
                streams.push((depth, captures));
            }
        }

        sweep_captures(streams, range.end)
    }
}

/// 在单层树上执行高亮查询，把 capture 裁剪到查询范围并保留 Tree-sitter 的文档顺序。
fn collect_capture_ranges(
    language: &Language,
    tree: &tree_sitter::Tree,
    range: &Range<usize>,
    text: &Snapshot,
    snapshot: &SyntaxSnapshot,
) -> Option<Vec<CaptureRange>> {
    if range.start >= range.end {
        return None;
    }
    // 局部 capture index → 快照全局 index 的映射在解析时构建，这里循环外取一次。
    let capture_table = snapshot.capture_index_table(language)?;
    // 无高亮查询的语言不产出 capture。
    let highlights = language.highlights()?;
    let mut cursor = QueryCursorHandle::new();
    cursor.set_byte_range(range.clone());
    let mut ranges = Vec::new();
    let mut captures = cursor.captures(highlights, tree.root_node(), SnapshotTextProvider(text));
    while let Some((query_match, capture_index)) = captures.next() {
        let capture = query_match.captures[*capture_index];
        let capture_range = capture.node.byte_range();
        let start = capture_range.start.max(range.start);
        let end = capture_range.end.min(range.end);
        let Some(name) = language.capture_names().get(capture.index as usize) else {
            continue;
        };
        if name.starts_with('_') {
            continue;
        }
        // 注入层的 capture 也经此表映射，渲染侧统一查全局表。
        let Some(&global_capture) = capture_table.get(capture.index as usize) else {
            continue;
        };
        if start < end {
            ranges.push(CaptureRange {
                start,
                end,
                capture: global_capture,
            });
        }
    }
    Some(ranges)
}

/// k 路归并各层 capture 起点，用活动栈直接产出非重叠 spans。
fn sweep_captures(streams: Vec<(u32, Vec<CaptureRange>)>, range_end: usize) -> Vec<HighlightSpan> {
    let mut heap: BinaryHeap<(Reverse<QueueKey>, usize)> = BinaryHeap::new();
    for (seq, (depth, stream)) in streams.iter().enumerate() {
        if let Some(capture) = stream.first() {
            heap.push((Reverse(QueueKey::new(capture, *depth, seq)), seq));
        }
    }
    let mut cursors = vec![0usize; streams.len()];
    let mut stack: Vec<(usize, u32)> = Vec::new();
    let mut spans: Vec<HighlightSpan> = Vec::new();
    let mut offset = heap
        .peek()
        .map(|(Reverse(key), _)| key.start)
        .unwrap_or(range_end);

    while let Some((Reverse(key), seq)) = heap.pop() {
        emit_until(key.start, &mut offset, &mut stack, &mut spans);

        let capture = streams[seq].1[cursors[seq]];
        stack.push((capture.end, capture.capture));

        // 推进该层游标。
        cursors[seq] += 1;
        if let Some(next) = streams[seq].1.get(cursors[seq]) {
            heap.push((Reverse(QueueKey::new(next, streams[seq].0, seq)), seq));
        }
    }

    emit_until(range_end, &mut offset, &mut stack, &mut spans);
    spans
}

fn emit_until(
    target: usize,
    offset: &mut usize,
    stack: &mut Vec<(usize, u32)>,
    spans: &mut Vec<HighlightSpan>,
) {
    while *offset < target {
        while stack.last().is_some_and(|(end, _)| *end <= *offset) {
            stack.pop();
        }
        let Some(&(end, capture)) = stack.last() else {
            *offset = target;
            break;
        };
        let span_end = end.min(target);
        if let Some(last) = spans.last_mut()
            && last.range.end == *offset
            && last.capture == capture
        {
            last.range.end = span_end;
        } else {
            spans.push(HighlightSpan {
                range: *offset..span_end,
                capture,
            });
        }
        *offset = span_end;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::test::{parsed_syntax, rust_buffer};

    fn capture_names_for(path: &str, source: &str) -> BTreeSet<String> {
        let (buffer, syntax) = parsed_syntax(path, source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        let names = syntax.capture_names();
        let spans = syntax.highlights(0..snapshot.len_bytes().get(), &snapshot);
        for pair in spans.windows(2) {
            assert!(
                pair[0].range.end <= pair[1].range.start,
                "{path} 的相邻高亮 span 不应重叠"
            );
        }
        spans
            .iter()
            .map(|span| names[span.capture as usize].to_string())
            .collect()
    }

    #[test]
    fn double_capture_on_same_node_resolves_to_the_inner_span() {
        // 同一节点命中多个 pattern（function_item 与 identifier 都覆盖函数名）：
        // 归并扫描必须稳定产出内层 capture，且相邻 spans 不重叠。
        let source = "fn main() {}\n";
        let (buffer, syntax) = rust_buffer(source);
        let snapshot = buffer.snapshot();
        let syntax_snapshot = syntax.snapshot();
        let names = syntax_snapshot.capture_names();
        let spans = syntax_snapshot.highlights(0..snapshot.len_bytes().get(), &snapshot);

        let main = source.find("main").unwrap();
        let covering = spans
            .iter()
            .filter(|span| span.range.start <= main && main < span.range.end)
            .collect::<Vec<_>>();
        assert_eq!(covering.len(), 1, "函数名区间只应被一个 span 覆盖");
        assert_eq!(
            names[covering[0].capture as usize].as_ref(),
            "function.definition",
            "同节点双捕获应解析为函数名 capture"
        );
        // 相邻 spans 无重叠（含同偏移 End/Start 邻接）。
        for pair in spans.windows(2) {
            assert!(
                pair[0].range.end <= pair[1].range.start,
                "相邻高亮 spans 不应重叠"
            );
        }
    }

    #[test]
    fn highlights_rust_captures_in_unicode_text() {
        let (buffer, syntax) = rust_buffer("fn 问候() { let 文本 = \"你好\"; }\n");
        let snapshot = buffer.snapshot();
        let syntax_snapshot = syntax.snapshot();
        let names = syntax_snapshot.capture_names();
        let spans = syntax_snapshot.highlights(0..snapshot.len_bytes().get(), &snapshot);

        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "keyword")
        );
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "function.definition")
        );
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "string")
        );
        assert!(
            spans
                .iter()
                .all(|span| span.range.end <= snapshot.len_bytes().get())
        );
    }

    #[test]
    fn project_queries_highlight_representative_language_constructs() {
        let cases: &[(&str, &str, &[&str])] = &[
            (
                "main.rs",
                "fn main() { let enabled = true; let count = 3; }\n",
                &["function.definition", "boolean", "number"],
            ),
            (
                "main.py",
                "@decorator\ndef greet(name: str) -> str:\n    return f\"Hi {name}\"\n",
                &["function.decorator", "function.definition", "type.builtin"],
            ),
            (
                "main.js",
                "const count = 3;\nconsole.log(count);\n",
                &["keyword.declaration", "number", "function.method"],
            ),
            (
                "view.jsx",
                "const view = <Button disabled={true}>Hi</Button>;\n",
                &["tag.component.jsx", "attribute.jsx", "boolean", "text.jsx"],
            ),
            (
                "main.ts",
                "interface User { name: string }\nconst user: User = { name: \"A\" };\n",
                &["type", "type.builtin", "property"],
            ),
            (
                "view.tsx",
                "const view = <Button disabled={true}>Hi</Button>;\n",
                &["tag.component.jsx", "attribute.jsx", "boolean"],
            ),
            (
                "main.c",
                "int main(void) { const int count = 3; return count; }\n",
                &["type", "function", "keyword", "number"],
            ),
            (
                "main.cpp",
                "class Greeter { public: const char *greet() { return \"hi\"; } };\n",
                &["keyword", "type", "function.definition", "string"],
            ),
            (
                "Program.cs",
                "public class Program { static int Main() { return 0; } }\n",
                &["keyword", "type", "function", "number"],
            ),
            (
                "main.go",
                "package main\nfunc greet(name string) string { return \"Hi \" + name }\n",
                &["keyword", "function", "type", "string"],
            ),
            (
                "app.rb",
                "class Greeter\n  def greet(name)\n    \"Hi #{name}\"\n  end\nend\n",
                &["keyword", "function.method", "variable.parameter", "string"],
            ),
            (
                "index.php",
                "<?php function greet(string $name): string { return \"Hi $name\"; }\n",
                &["keyword", "function", "type.builtin", "string"],
            ),
            (
                "main.swift",
                "struct Greeter { func greet(name: String) -> String { return \"Hi\" } }\n",
                &["keyword.type", "keyword.function", "type", "string"],
            ),
            (
                "Main.kt",
                "class Greeter { fun greet(name: String): String { return \"Hi $name\" } }\n",
                &["keyword", "function.definition", "type", "string"],
            ),
            (
                "init.lua",
                "local function greet(name) return \"Hi \" .. name end\n",
                &["keyword", "function", "parameter", "string"],
            ),
            (
                "main.zig",
                "const std = @import(\"std\"); pub fn main() void { std.debug.print(\"hi\", .{}); }\n",
                &["keyword", "function", "type.builtin", "string"],
            ),
            (
                "query.sql",
                "SELECT name FROM users WHERE active = TRUE AND count > 3;\n",
                &["keyword", "field", "boolean", "number"],
            ),
            (
                "data.json",
                "{\"enabled\": true, \"count\": 3}\n",
                &["property.json_key", "boolean", "number"],
            ),
            (
                "data.yaml",
                "enabled: true\ncount: 3\n",
                &["property", "boolean", "number"],
            ),
            (
                "Main.java",
                "@Deprecated public class Main { static final int MAX = 3; String greet(String name) { return \"Hi \" + name; } }\n",
                &[
                    "attribute",
                    "type",
                    "type.builtin",
                    "function.method",
                    "number",
                ],
            ),
            (
                "script.sh",
                "#!/usr/bin/env bash\nfunction greet() { local name=\"$1\"; echo \"Hi $name\"; }\n",
                &["keyword.directive", "keyword", "function", "variable"],
            ),
            (
                "Cargo.toml",
                "[package]\nname = \"zcv\"\nenabled = true\ncount = 3\npublished = 2026-08-31\n",
                &[
                    "type",
                    "property",
                    "string",
                    "boolean",
                    "number",
                    "string.special",
                ],
            ),
            (
                "README.md",
                "# 标题\n\n**加粗** 和 *强调*，另见 [链接](https://example.com)。\n",
                &["text.title", "text.strong", "text.emphasis", "text.uri"],
            ),
            (
                "index.html",
                "<!doctype html><main class=\"card\">Hello &amp;</main>\n",
                &[
                    "tag.doctype",
                    "tag",
                    "attribute",
                    "string",
                    "string.special",
                ],
            ),
            (
                "main.css",
                ".card:hover { color: #fff; margin: 1rem; --gap: 2px; }\n",
                &[
                    "selector.class",
                    "selector.pseudo",
                    "property",
                    "string.special",
                    "number",
                    "type.unit",
                    "variable",
                ],
            ),
        ];

        for (path, source, expected) in cases {
            let captures = capture_names_for(path, source);
            for expected in *expected {
                assert!(
                    captures.contains(*expected),
                    "{path} 应产生 `{expected}`，实际为 {captures:?}"
                );
            }
            assert!(
                captures.iter().all(|name| !name.starts_with('_')),
                "{path} 不应把查询辅助 capture 暴露给渲染层"
            );
        }
    }

    #[test]
    fn markdown_inline_layer_overrides_block_highlights() {
        let (buffer, syntax) = parsed_syntax("README.md", "普通 *强调* 和 **加粗**\n");
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        assert!(
            syntax
                .injection_layers()
                .iter()
                .any(|layer| layer.language.name() == "Markdown Inline")
        );
        let names = syntax.capture_names();
        let spans = syntax.highlights(0..snapshot.len_bytes().get(), &snapshot);
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "text.emphasis")
        );
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "text.strong")
        );
    }

    #[test]
    fn html_injects_css_and_javascript_layers() {
        let source = "<style>.item { color: red; }</style><script>let value = 1;</script>";
        let (buffer, syntax) = parsed_syntax("index.html", source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        assert!(
            syntax
                .injection_layers()
                .iter()
                .any(|layer| layer.language.name() == "CSS")
        );
        assert!(
            syntax
                .injection_layers()
                .iter()
                .any(|layer| layer.language.name() == "JavaScript")
        );
        let names = syntax.capture_names();
        let spans = syntax.highlights(0..snapshot.len_bytes().get(), &snapshot);
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "property")
        );
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "keyword.declaration")
        );
    }

    #[test]
    fn baseline_languages_inject_registered_nested_languages() {
        for (path, source, expected) in [
            ("main.c", "#define VALUE (1 + 2)\n", "C"),
            (
                "main.cpp",
                "const char *query = R\"sql(SELECT name FROM users)sql\";\n",
                "SQL",
            ),
            (
                "index.php",
                "<?php\n$query = <<<SQL\nSELECT name FROM users;\nSQL;\n",
                "SQL",
            ),
            (
                "init.lua",
                "ffi.cdef[[int add(int left, int right);]]\n",
                "C",
            ),
        ] {
            let (_, syntax) = parsed_syntax(path, source);
            assert!(
                syntax
                    .snapshot()
                    .injection_layers()
                    .iter()
                    .any(|layer| layer.language.name() == expected),
                "{path} 应注入 {expected}"
            );
        }
    }

    #[test]
    fn javascript_tagged_template_injects_css_highlights() {
        let source = "const styles = css`.item { color: red; }`;\n";
        let (buffer, syntax) = parsed_syntax("styles.js", source);
        let snapshot = buffer.snapshot();
        let syntax = syntax.snapshot();
        assert!(
            syntax
                .injection_layers()
                .iter()
                .any(|layer| layer.language.name() == "CSS")
        );
        let names = syntax.capture_names();
        let spans = syntax.highlights(0..snapshot.len_bytes().get(), &snapshot);
        assert!(
            spans
                .iter()
                .any(|span| names[span.capture as usize].as_ref() == "property")
        );
    }
}
