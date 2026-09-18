use std::path::Path;
use std::sync::Arc;

use zcv_language::LanguageRegistry;

struct Fixture {
    file: &'static str,
    language: &'static str,
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        file: "rust.rs",
        language: "Rust",
    },
    Fixture {
        file: "main.c",
        language: "C",
    },
    Fixture {
        file: "main.cpp",
        language: "C++",
    },
    Fixture {
        file: "Program.cs",
        language: "C#",
    },
    Fixture {
        file: "main.go",
        language: "Go",
    },
    Fixture {
        file: "main.py",
        language: "Python",
    },
    Fixture {
        file: "main.js",
        language: "JavaScript",
    },
    Fixture {
        file: "view.jsx",
        language: "JSX",
    },
    Fixture {
        file: "main.ts",
        language: "TypeScript",
    },
    Fixture {
        file: "view.tsx",
        language: "TSX",
    },
    Fixture {
        file: "Main.java",
        language: "Java",
    },
    Fixture {
        file: "Main.kt",
        language: "Kotlin",
    },
    Fixture {
        file: "script.sh",
        language: "Shell",
    },
    Fixture {
        file: "app.rb",
        language: "Ruby",
    },
    Fixture {
        file: "index.php",
        language: "PHP",
    },
    Fixture {
        file: "main.swift",
        language: "Swift",
    },
    Fixture {
        file: "init.lua",
        language: "Lua",
    },
    Fixture {
        file: "main.zig",
        language: "Zig",
    },
    Fixture {
        file: "query.sql",
        language: "SQL",
    },
    Fixture {
        file: "sample.toml",
        language: "TOML",
    },
    Fixture {
        file: "data.json",
        language: "JSON",
    },
    Fixture {
        file: "data.yaml",
        language: "YAML",
    },
    Fixture {
        file: "markdown.md",
        language: "Markdown",
    },
    Fixture {
        file: "index.html",
        language: "HTML",
    },
    Fixture {
        file: "main.css",
        language: "CSS",
    },
];

/// 端到端覆盖文件识别与公开的代码片段高亮入口。
///
/// 各语言的具体 capture 契约由 `src/highlighting.rs` 的单元测试负责，
/// 这里只确认每个 fixture 都能经 `LanguageRegistry` 识别出语言，并经
/// `highlight_snippet_with_cancellation` 产出高亮跨度，避免同一份 capture
/// 明细在单元测试与集成测试两处重复维护。
#[test]
fn local_language_files_are_recognized_and_highlighted() {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let registry = Arc::new(LanguageRegistry::new());
    for fixture in FIXTURES {
        let path = Path::new(fixture.file);
        let source = std::fs::read_to_string(fixture_dir.join(fixture.file))
            .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", fixture.file));
        let language = registry.language_for_file(path, source.lines().next());
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
        let highlights = zcv_language::highlight_snippet_with_cancellation(
            &registry,
            extension,
            &source,
            &zcv_language::SnippetHighlightCancellation::default(),
        )
        .unwrap_or_else(|| panic!("{} 应产生高亮", fixture.file));
        assert!(
            !highlights.spans.is_empty(),
            "{} 应产生高亮跨度",
            fixture.file
        );
    }
}
