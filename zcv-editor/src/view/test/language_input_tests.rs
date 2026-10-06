//! 逐语言代表性输入行为测试。
//!
//! 对已在覆盖表中声明与 Zed 对齐的语言，分别验证默认代码、字符串、注释与注入语境下的
//! 输入结果与光标；断言最终文本与光标，不检查查询结果或内部节点。

use gpui::TestAppContext;

use super::jsx_tag_tests::{buffer_text, editor_with_state, state_text, type_text};

/// 在给定初始状态输入 `input`，比较带光标标记的最终状态。
fn check(cx: &mut TestAppContext, path: &'static str, before: &str, input: &str, after: &str) {
    let (buffer, editor, cx) = editor_with_state(cx, path, before);
    cx.run_until_parked();
    type_text(&editor, cx, input);
    let expected_text = after.replace('ˇ', "");
    assert_eq!(
        buffer_text(&buffer, cx),
        expected_text,
        "文本不一致：{path} {before:?}"
    );
    assert_eq!(
        state_text(&editor, cx),
        after,
        "光标不一致：{path} {before:?}"
    );
}

#[gpui::test]
fn code_scope_autocloses_pairs_for_aligned_languages(cx: &mut TestAppContext) {
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "test.rs",
            "fn main() { let x = ˇ }\n",
            "(",
            "fn main() { let x = (ˇ) }\n",
        ),
        ("test.c", "int x = ˇ;\n", "(", "int x = (ˇ);\n"),
        ("test.cpp", "int x = ˇ;\n", "(", "int x = (ˇ);\n"),
        (
            "test.go",
            "func f() { x := ˇ }\n",
            "(",
            "func f() { x := (ˇ) }\n",
        ),
        ("test.py", "x = ˇ\n", "(", "x = (ˇ)\n"),
        ("test.sh", "x=ˇ\n", "(", "x=(ˇ)\n"),
        ("test.json", "{\"a\": ˇ}", "[", "{\"a\": [ˇ]}"),
        ("test.yaml", "a: ˇ\n", "[", "a: [ˇ]\n"),
        ("test.css", "a { color: ˇ }\n", "(", "a { color: (ˇ) }\n"),
        ("test.html", "<p>ˇ </p>\n", "(", "<p>(ˇ) </p>\n"),
        ("test.js", "const x = ˇ;\n", "(", "const x = (ˇ);\n"),
        ("test.jsx", "const x = ˇ;\n", "(", "const x = (ˇ);\n"),
        ("test.ts", "const x = ˇ;\n", "(", "const x = (ˇ);\n"),
        ("test.tsx", "const x = ˇ;\n", "(", "const x = (ˇ);\n"),
        ("test.md", "ˇ\n", "(", "(ˇ)\n"),
    ];
    for (path, before, input, after) in cases {
        check(cx, path, before, input, after);
    }
}

#[gpui::test]
fn string_scope_disables_same_quote_for_aligned_languages(cx: &mut TestAppContext) {
    let cases: &[(&str, &str, &str, &str)] = &[
        (
            "test.rs",
            "let s = \"aˇb\";\n",
            "\"",
            "let s = \"a\"ˇb\";\n",
        ),
        (
            "test.c",
            "char *s = \"aˇb\";\n",
            "\"",
            "char *s = \"a\"ˇb\";\n",
        ),
        (
            "test.cpp",
            "char *s = \"aˇb\";\n",
            "\"",
            "char *s = \"a\"ˇb\";\n",
        ),
        ("test.go", "s := \"aˇb\"\n", "\"", "s := \"a\"ˇb\"\n"),
        ("test.py", "s = \"aˇb\"\n", "\"", "s = \"a\"ˇb\"\n"),
        ("test.sh", "x=\"aˇb\"\n", "\"", "x=\"a\"ˇb\"\n"),
        ("test.json", "{\"a\": \"xˇy\"}", "\"", "{\"a\": \"x\"ˇy\"}"),
        ("test.yaml", "a: \"xˇy\"\n", "\"", "a: \"x\"ˇy\"\n"),
        (
            "test.css",
            "a { content: \"xˇy\"; }\n",
            "\"",
            "a { content: \"x\"ˇy\"; }\n",
        ),
        (
            "test.html",
            "<p title=\"xˇy\">\n",
            "\"",
            "<p title=\"x\"ˇy\">\n",
        ),
        (
            "test.js",
            "const s = \"aˇb\";\n",
            "\"",
            "const s = \"a\"ˇb\";\n",
        ),
        (
            "test.jsx",
            "const s = \"aˇb\";\n",
            "\"",
            "const s = \"a\"ˇb\";\n",
        ),
        (
            "test.ts",
            "const s = \"aˇb\";\n",
            "\"",
            "const s = \"a\"ˇb\";\n",
        ),
        (
            "test.tsx",
            "const s = \"aˇb\";\n",
            "\"",
            "const s = \"a\"ˇb\";\n",
        ),
    ];
    for (path, before, input, after) in cases {
        check(cx, path, before, input, after);
    }
}

#[gpui::test]
fn comment_scope_keeps_bracket_pairs_for_aligned_languages(cx: &mut TestAppContext) {
    let cases: &[(&str, &str, &str, &str)] = &[
        ("test.rs", "// ˇ\n", "(", "// (ˇ)\n"),
        ("test.c", "// ˇ\n", "(", "// (ˇ)\n"),
        ("test.cpp", "// ˇ\n", "(", "// (ˇ)\n"),
        ("test.go", "// ˇ\n", "(", "// (ˇ)\n"),
        ("test.py", "# ˇ\n", "(", "# (ˇ)\n"),
        ("test.sh", "# ˇ\n", "(", "# (ˇ)\n"),
        ("test.yaml", "# ˇ\n", "{", "# {ˇ}\n"),
        ("test.css", "/* ˇ */\n", "(", "/* (ˇ) */\n"),
        ("test.html", "<!-- ˇ -->\n", "(", "<!-- (ˇ) -->\n"),
        ("test.js", "// ˇ\n", "(", "// (ˇ)\n"),
        ("test.jsx", "// ˇ\n", "(", "// (ˇ)\n"),
        ("test.ts", "// ˇ\n", "(", "// (ˇ)\n"),
        ("test.tsx", "// ˇ\n", "(", "// (ˇ)\n"),
    ];
    for (path, before, input, after) in cases {
        check(cx, path, before, input, after);
    }
}

#[gpui::test]
fn markdown_injected_rust_uses_inner_language_pairs(cx: &mut TestAppContext) {
    check(
        cx,
        "test.md",
        "```rust\nlet x = ˇ\n```\n",
        "(",
        "```rust\nlet x = (ˇ)\n```\n",
    );
}
