use std::collections::HashSet;

use super::*;

#[test]
fn detects_rust_and_tsx_with_distinct_grammars() {
    assert_eq!(
        LanguageRegistry::new()
            .language_for_file(Path::new("main.rs"), None)
            .unwrap()
            .name(),
        "Rust"
    );
    assert_eq!(
        LanguageRegistry::new()
            .language_for_file(Path::new("view.tsx"), None)
            .unwrap()
            .name(),
        "TSX"
    );
}

#[test]
fn detects_baseline_languages_by_suffix() {
    for (path, expected) in [
        ("main.c", "C"),
        ("main.cpp", "C++"),
        ("main.hpp", "C++"),
        ("Program.cs", "C#"),
        ("main.go", "Go"),
        ("app.rb", "Ruby"),
        ("index.php", "PHP"),
        ("main.swift", "Swift"),
        ("Main.kt", "Kotlin"),
        ("build.gradle.kts", "Kotlin"),
        ("init.lua", "Lua"),
        ("main.zig", "Zig"),
        ("query.sql", "SQL"),
    ] {
        assert_eq!(
            LanguageRegistry::new()
                .language_for_file(Path::new(path), None)
                .unwrap()
                .name(),
            expected,
            "{path} 应识别为 {expected}"
        );
    }
}

#[test]
fn detects_baseline_script_languages_from_first_line() {
    for (first_line, expected) in [
        ("#!/usr/bin/env ruby", "Ruby"),
        ("#!/usr/bin/php", "PHP"),
        ("#!/usr/bin/env swift", "Swift"),
        ("#!/usr/bin/env lua5.4", "Lua"),
        ("//usr/bin/env go run $0 $@; exit", "Go"),
    ] {
        assert_eq!(
            LanguageRegistry::new()
                .language_for_file(Path::new("script"), Some(first_line))
                .unwrap()
                .name(),
            expected,
            "`{first_line}` 应识别为 {expected}"
        );
    }
}

#[test]
fn reuses_loaded_language_and_compiled_queries() {
    let registry = LanguageRegistry::new();
    let first = registry
        .language_for_file(Path::new("main.rs"), None)
        .unwrap();
    let second = registry
        .language_for_file(Path::new("lib.rs"), None)
        .unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert!(Arc::ptr_eq(
        first.highlights().unwrap(),
        second.highlights().unwrap()
    ));
}

#[test]
fn detects_shell_from_shebang() {
    assert_eq!(
        LanguageRegistry::new()
            .language_for_file(Path::new("script"), Some("#!/usr/bin/env bash"))
            .unwrap()
            .name(),
        "Shell"
    );
}

#[test]
fn unknown_files_fall_back_to_plain_text() {
    // 未支持的语言后缀也必须明确回落为纯文本，不能注册成缺少 grammar 的半支持语言。
    for path in ["main.dart", "main.ex", "main.hs", "query.graphql"] {
        let language = LanguageRegistry::new()
            .language_for_file(Path::new(path), None)
            .unwrap();
        assert_eq!(language.name(), "纯文本", "{path} 尚未提供完整语言规格");
        assert!(language.grammar().is_none());
    }

    // .gitignore 等无扩展名文件与未知后缀都以纯文本兜底，语言名始终可显示。
    assert_eq!(
        LanguageRegistry::new()
            .language_for_file(Path::new(".gitignore"), None)
            .unwrap()
            .name(),
        "纯文本"
    );
    assert_eq!(
        LanguageRegistry::new()
            .language_for_file(Path::new("Makefile"), None)
            .unwrap()
            .name(),
        "纯文本"
    );
    assert_eq!(
        LanguageRegistry::new()
            .language_for_file(Path::new("archive.unknown_ext"), None)
            .unwrap()
            .name(),
        "纯文本"
    );
    // .txt 显式匹配 Plain Text；无语法树语言不产出高亮查询。
    let plain = LanguageRegistry::new()
        .language_for_file(Path::new("notes.txt"), None)
        .unwrap();
    assert_eq!(plain.name(), "纯文本");
    assert!(plain.highlights().is_none(), "纯文本语言不应有高亮查询");
}

#[test]
fn javascript_family_compiles_declared_query_layers() {
    let jsx = LanguageRegistry::new()
        .language_for_file(Path::new("view.jsx"), None)
        .unwrap();
    assert!(
        jsx.highlights()
            .unwrap()
            .capture_names()
            .contains(&"variable")
    );
    assert!(
        jsx.highlights()
            .unwrap()
            .capture_names()
            .contains(&"tag.jsx")
    );
    assert!(jsx.injections().is_some());

    let typescript = LanguageRegistry::new()
        .language_for_file(Path::new("main.ts"), None)
        .unwrap();
    assert!(
        typescript
            .highlights()
            .unwrap()
            .capture_names()
            .contains(&"variable")
    );
    assert!(
        typescript
            .highlights()
            .unwrap()
            .capture_names()
            .contains(&"type")
    );
    assert!(typescript.injections().is_some());

    let tsx = LanguageRegistry::new()
        .language_for_file(Path::new("view.tsx"), None)
        .unwrap();
    assert!(
        tsx.highlights()
            .unwrap()
            .capture_names()
            .contains(&"variable")
    );
    assert!(
        tsx.highlights()
            .unwrap()
            .capture_names()
            .contains(&"tag.jsx")
    );
    assert!(tsx.highlights().unwrap().capture_names().contains(&"type"));
    assert!(tsx.injections().is_some());
}

#[test]
fn outline_queries_are_optional_and_use_the_declared_capture_contract() {
    for (path, expected) in [
        ("main.rs", true),
        ("main.py", true),
        ("main.js", true),
        ("main.jsx", true),
        ("main.ts", true),
        ("main.tsx", true),
        ("main.go", false),
        ("main.c", false),
        ("main.cpp", false),
        ("README.md", true),
        ("index.html", true),
        ("notes.txt", false),
    ] {
        let language = LanguageRegistry::new()
            .language_for_file(Path::new(path), None)
            .unwrap();
        assert_eq!(
            language.outline().is_some(),
            expected,
            "{path} 大纲查询能力错误"
        );
        if let Some(query) = language.outline() {
            assert!(query.capture_names().iter().all(|name| {
                matches!(
                    *name,
                    "item" | "name" | "context" | "context.extra" | "annotation" | "open" | "close"
                )
            }));
        }
        let locals_expected = matches!(
            path,
            "main.rs"
                | "main.py"
                | "main.js"
                | "main.jsx"
                | "main.ts"
                | "main.tsx"
                | "main.go"
                | "main.c"
                | "main.cpp"
        );
        assert_eq!(
            language.locals().is_some(),
            locals_expected,
            "{path} 局部语义查询能力错误"
        );
        if let Some(query) = language.locals() {
            assert!(query.capture_names().iter().all(|name| {
                matches!(
                    *name,
                    "local.scope" | "local.definition" | "local.reference"
                )
            }));
        }
    }
}

#[test]
fn builtin_language_specs_are_complete_unique_and_reachable() {
    let registry = LanguageRegistry::new();
    let mut plain_text_count = 0;
    let mut names = HashSet::new();
    let mut suffixes = HashSet::new();
    for spec in &registry.languages {
        assert!(
            names.insert(spec.name.to_ascii_lowercase()),
            "语言名 `{}` 不能重复",
            spec.name
        );
        for suffix in spec.matcher.suffixes {
            assert!(
                suffixes.insert(suffix.to_ascii_lowercase()),
                "文件后缀 `{suffix}` 不能由多个语言规格声明"
            );
        }
        assert!(
            !spec.matcher.suffixes.is_empty()
                || spec.matcher.first_line_pattern.is_some()
                || spec.injection_alias.is_some(),
            "{} 必须能通过文件、首行或注入别名到达",
            spec.name
        );

        let language = registry.load(spec);
        match spec.support {
            LanguageSupportSpec::PlainText => {
                plain_text_count += 1;
                assert!(language.grammar().is_none());
                assert!(language.highlights().is_none());
            }
            LanguageSupportSpec::TreeSitter { .. } => {
                assert!(
                    language.grammar().is_some(),
                    "{} 必须加载 grammar",
                    spec.name
                );
                assert!(
                    language.highlights().is_some(),
                    "{} 必须加载高亮查询",
                    spec.name
                );
                assert!(
                    !language.capture_names().is_empty(),
                    "{} 的高亮查询必须声明 capture",
                    spec.name
                );
                if !spec.matcher.suffixes.is_empty() || spec.matcher.first_line_pattern.is_some() {
                    assert!(
                        language.brackets().is_some(),
                        "{} 必须提供括号查询",
                        spec.name
                    );
                    assert!(
                        language.indents().is_some(),
                        "{} 必须提供缩进查询",
                        spec.name
                    );
                    assert!(language.folds().is_some(), "{} 必须提供折叠查询", spec.name);
                }
            }
        }
    }
    assert_eq!(plain_text_count, 1, "注册表只能有一个纯文本兜底规格");
}

#[test]
fn languages_declare_their_extra_word_characters() {
    let registry = LanguageRegistry::new();
    let javascript = registry
        .language_for_file(Path::new("main.js"), None)
        .unwrap();
    assert!(
        javascript.word_boundary().is_identifier_continue('$'),
        "JavaScript 应把 $ 视为词字符"
    );
    assert!(
        javascript.word_boundary().is_identifier_continue('#'),
        "JavaScript 应把 # 视为词字符"
    );
    let rust = registry
        .language_for_file(Path::new("main.rs"), None)
        .unwrap();
    assert!(
        !rust.word_boundary().is_identifier_continue('$'),
        "Rust 不应把 $ 视为词字符"
    );
    assert!(
        rust.word_boundary().is_identifier_continue('_'),
        "下划线始终是词字符"
    );
}

#[test]
fn loaded_languages_declare_input_autoclose_pairs() {
    for path in ["main.rs", "main.py", "data.json", "README.md", "style.css"] {
        let language = LanguageRegistry::new()
            .language_for_file(Path::new(path), None)
            .unwrap_or_else(|| panic!("{path} 应加载语言"));
        let pairs = language.auto_close_pairs();
        assert!(!pairs.is_empty(), "{path} 应声明输入自动闭合配对");
        assert!(
            pairs
                .iter()
                .any(|pair| pair.start == "(" && pair.end == ")"),
            "{path} 应含括号对"
        );
    }
}
