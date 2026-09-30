use std::collections::BTreeSet;

use crate::highlight_cache::HighlightCache;
use crate::test::{parsed_syntax, rust_buffer};

fn capture_names_for(path: &str, source: &str) -> BTreeSet<String> {
    let (buffer, syntax) = parsed_syntax(path, source);
    let snapshot = buffer.snapshot();
    let syntax = syntax.snapshot();
    let names = syntax.capture_names();
    let spans = syntax.highlights(
        0..snapshot.len_bytes().get(),
        &snapshot,
        &HighlightCache::new(),
    );
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
    let spans = syntax_snapshot.highlights(
        0..snapshot.len_bytes().get(),
        &snapshot,
        &HighlightCache::new(),
    );

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
    let spans = syntax_snapshot.highlights(
        0..snapshot.len_bytes().get(),
        &snapshot,
        &HighlightCache::new(),
    );

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
fn markdown_heading_at_fixed_byte_boundary_keeps_utf8_highlights() {
    let source = format!("{}\n## 测试\n", "x".repeat(4091));
    let heading = source.find("测试").unwrap();
    assert_eq!(heading, 4095);
    let (buffer, syntax) = parsed_syntax("README.md", &source);
    let snapshot = buffer.snapshot();
    let syntax = syntax.snapshot();
    let cache = HighlightCache::new();
    let item = syntax
        .outline(0..snapshot.len_bytes().get(), &snapshot)
        .into_iter()
        .find(|item| item.name == "测试")
        .expect("跨字节边界的标题应进入大纲");

    for _ in 0..2 {
        let spans = syntax.highlights(heading..heading + "测试".len(), &snapshot, &cache);
        assert!(!spans.is_empty());
        for span in spans {
            assert!(source.is_char_boundary(span.range.start), "{span:?}");
            assert!(source.is_char_boundary(span.range.end), "{span:?}");
        }
        for part in &item.text_ranges {
            for span in syntax.highlights(part.source_range.clone(), &snapshot, &cache) {
                let start = part.text_range.start + span.range.start - part.source_range.start;
                let end = part.text_range.start + span.range.end - part.source_range.start;
                assert!(item.text.is_char_boundary(start), "{span:?}");
                assert!(item.text.is_char_boundary(end), "{span:?}");
            }
        }
    }
}

#[test]
fn highlights_span_adjacent_row_chunks() {
    let source = format!("{}## 甲\n## 乙\n", "x\n".repeat(49));
    let first = source.find('甲').unwrap();
    let second = source.find('乙').unwrap();
    let (buffer, syntax) = parsed_syntax("README.md", &source);
    let snapshot = buffer.snapshot();
    let spans = syntax.snapshot().highlights(
        first..second + '乙'.len_utf8(),
        &snapshot,
        &HighlightCache::new(),
    );

    for name in [first, second] {
        assert!(
            spans
                .iter()
                .any(|span| span.range.start <= name && name < span.range.end),
            "跨行块查询应保留两侧标题的高亮"
        );
    }
    for span in spans {
        assert!(source.is_char_boundary(span.range.start));
        assert!(source.is_char_boundary(span.range.end));
    }
}

#[test]
fn long_row_chunk_highlights_requested_rust_symbol() {
    let source = format!("// {}\nfn 测试() {{}}\n", "x".repeat(70 * 1024));
    let name = source.find("测试").unwrap();
    let (buffer, syntax) = rust_buffer(&source);
    let snapshot = buffer.snapshot();
    let spans =
        syntax
            .snapshot()
            .highlights(name..name + "测试".len(), &snapshot, &HighlightCache::new());

    assert!(spans.iter().any(|span| span.range.start == name));
    assert!(spans.iter().all(|span| {
        source.is_char_boundary(span.range.start) && source.is_char_boundary(span.range.end)
    }));
}

#[test]
fn project_queries_highlight_representative_language_constructs() {
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "main.rs",
            "fn main() { let enabled = true; let count = 3; }\n",
            &["keyword", "function.definition", "boolean", "number"],
        ),
        (
            "main.py",
            "@decorator\ndef greet(name: str) -> str:\n    return f\"Hi {name}\"\n",
            &[
                "keyword",
                "function.decorator",
                "function.definition",
                "type.builtin",
                "string",
            ],
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
            .layers_for_range(&snapshot, &(0..snapshot.len_bytes().get()))
            .any(|layer| layer.language.name() == "Markdown Inline")
    );
    let names = syntax.capture_names();
    let spans = syntax.highlights(
        0..snapshot.len_bytes().get(),
        &snapshot,
        &HighlightCache::new(),
    );
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
            .layers_for_range(&snapshot, &(0..snapshot.len_bytes().get()))
            .any(|layer| layer.language.name() == "CSS")
    );
    assert!(
        syntax
            .layers_for_range(&snapshot, &(0..snapshot.len_bytes().get()))
            .any(|layer| layer.language.name() == "JavaScript")
    );
    let names = syntax.capture_names();
    let spans = syntax.highlights(
        0..snapshot.len_bytes().get(),
        &snapshot,
        &HighlightCache::new(),
    );
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
fn html_custom_components_have_component_highlights() {
    let (buffer, syntax) = parsed_syntax(
        "index.html",
        "<main><UserCard data-id=\"1\">你好</UserCard></main>\n",
    );
    let snapshot = buffer.snapshot();
    let syntax = syntax.snapshot();
    let names = syntax.capture_names();
    let spans = syntax.highlights(
        0..snapshot.len_bytes().get(),
        &snapshot,
        &HighlightCache::new(),
    );

    let component_start = "<main><".len();
    assert!(
        spans.iter().any(|span| {
            names[span.capture as usize].as_ref() == "tag.component"
                && span.range.start <= component_start
                && component_start < span.range.end
        }),
        "大写开头的 HTML 标签应识别为组件"
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
        let (buffer, syntax) = parsed_syntax(path, source);
        let snapshot = buffer.snapshot();
        assert!(
            syntax
                .snapshot()
                .layers_for_range(&snapshot, &(0..snapshot.len_bytes().get()))
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
            .layers_for_range(&snapshot, &(0..snapshot.len_bytes().get()))
            .any(|layer| layer.language.name() == "CSS")
    );
    let names = syntax.capture_names();
    let spans = syntax.highlights(
        0..snapshot.len_bytes().get(),
        &snapshot,
        &HighlightCache::new(),
    );
    assert!(
        spans
            .iter()
            .any(|span| names[span.capture as usize].as_ref() == "property")
    );
}
