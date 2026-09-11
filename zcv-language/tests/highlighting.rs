use std::path::Path;
use std::sync::Arc;

use zcv_language::{HighlightSpan, language_for_file};

struct Fixture {
    file: &'static str,
    language: &'static str,
    captures: &'static [&'static str],
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        file: "main.rs",
        language: "Rust",
        captures: &["keyword", "function.definition", "number"],
    },
    Fixture {
        file: "main.c",
        language: "C",
        captures: &["type", "function", "number"],
    },
    Fixture {
        file: "main.cpp",
        language: "C++",
        captures: &["keyword", "type", "function.definition"],
    },
    Fixture {
        file: "Program.cs",
        language: "C#",
        captures: &["keyword", "type", "function"],
    },
    Fixture {
        file: "main.go",
        language: "Go",
        captures: &["keyword", "function", "string"],
    },
    Fixture {
        file: "main.py",
        language: "Python",
        captures: &["keyword", "function.definition", "string"],
    },
    Fixture {
        file: "main.js",
        language: "JavaScript",
        captures: &["keyword.declaration", "function.method", "number"],
    },
    Fixture {
        file: "view.jsx",
        language: "JSX",
        captures: &["tag.component.jsx", "attribute.jsx", "boolean"],
    },
    Fixture {
        file: "main.ts",
        language: "TypeScript",
        captures: &["type", "type.builtin", "property"],
    },
    Fixture {
        file: "view.tsx",
        language: "TSX",
        captures: &["tag.component.jsx", "attribute.jsx", "boolean"],
    },
    Fixture {
        file: "Main.java",
        language: "Java",
        captures: &["attribute", "type", "function.method"],
    },
    Fixture {
        file: "Main.kt",
        language: "Kotlin",
        captures: &["keyword", "function.definition", "type"],
    },
    Fixture {
        file: "script.sh",
        language: "Shell",
        captures: &["keyword.directive", "function", "variable"],
    },
    Fixture {
        file: "app.rb",
        language: "Ruby",
        captures: &["keyword", "function.method", "variable.parameter"],
    },
    Fixture {
        file: "index.php",
        language: "PHP",
        captures: &["keyword", "function", "type.builtin"],
    },
    Fixture {
        file: "main.swift",
        language: "Swift",
        captures: &["keyword.type", "keyword.function", "type"],
    },
    Fixture {
        file: "init.lua",
        language: "Lua",
        captures: &["keyword", "function", "parameter"],
    },
    Fixture {
        file: "main.zig",
        language: "Zig",
        captures: &["keyword", "function", "type.builtin"],
    },
    Fixture {
        file: "query.sql",
        language: "SQL",
        captures: &["keyword", "field", "number"],
    },
    Fixture {
        file: "Cargo.toml",
        language: "TOML",
        captures: &["type", "property", "boolean"],
    },
    Fixture {
        file: "data.json",
        language: "JSON",
        captures: &["property.json_key", "boolean", "number"],
    },
    Fixture {
        file: "data.yaml",
        language: "YAML",
        captures: &["property", "boolean", "number"],
    },
    Fixture {
        file: "markdown.md",
        language: "Markdown",
        captures: &["text.title", "text.strong", "text.uri"],
    },
    Fixture {
        file: "index.html",
        language: "HTML",
        captures: &["tag", "attribute", "string.special"],
    },
    Fixture {
        file: "main.css",
        language: "CSS",
        captures: &["selector.class", "property", "type.unit"],
    },
];

fn capture_names(highlights: &[HighlightSpan], names: &[Arc<str>]) -> Vec<String> {
    highlights
        .iter()
        .map(|span| names[span.capture as usize].to_string())
        .collect()
}

#[test]
fn local_language_files_are_recognized_and_highlighted() {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../本地高亮测试.local");
    for fixture in FIXTURES {
        let path = Path::new(fixture.file);
        let source = std::fs::read_to_string(fixture_dir.join(fixture.file))
            .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", fixture.file));
        let language = language_for_file(path, source.lines().next());
        let language = language.unwrap_or_else(|| panic!("{} 应识别语言", fixture.file));
        assert_eq!(
            language.name(),
            fixture.language,
            "{} 语言识别错误",
            fixture.file
        );

        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .expect("高亮样例文件应有扩展名");
        let highlights = zcv_language::highlight_snippet(extension, &source)
            .unwrap_or_else(|| panic!("{} 应产生高亮", fixture.file));
        let actual = capture_names(&highlights.spans, &highlights.capture_names);
        for expected in fixture.captures {
            assert!(
                actual.iter().any(|actual| actual == expected),
                "{} 应产生 `{expected}`，实际为 {actual:?}",
                fixture.file
            );
        }
    }
}
