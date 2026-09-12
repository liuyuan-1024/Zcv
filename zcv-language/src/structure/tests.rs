//! 结构查询的跨语言行为测试。
//!
//! 测试通过公开的 `SyntaxSnapshot` 查询验证坐标、注入层、作用域和折叠边界，不依赖各模块的内部实现细节。

use super::NewlineIndent;
use crate::test::{parsed_syntax, rust_buffer};
use zcv_text::{ByteOffset, Edit, TransactionMetadata};

#[test]
fn rust_syntax_snapshot_exposes_zed_structure_queries() {
    let source = "struct Demo { value: i32 }\nfn main() { let x = (1 + 2); }\n";
    let (buffer, syntax) = rust_buffer(source);
    let snapshot = buffer.snapshot();
    let syntax = syntax.snapshot();
    let full = 0..snapshot.len_bytes().get();

    let brackets = syntax.bracket_pairs(full.clone(), &snapshot);
    assert!(
        brackets.iter().any(|pair| {
            &source[pair.open.clone()] == "(" && &source[pair.close.clone()] == ")"
        })
    );

    assert!(!syntax.indent_ranges(full, &snapshot).is_empty());
}

#[test]
fn outline_preserves_nested_same_named_unicode_definitions() {
    let source = "mod 数据 {\n    struct Item {\n        value: i32,\n    }\n    fn build() {\n        let value = 1;\n    }\n}\nfn build() {}\n";
    let (buffer, syntax) = parsed_syntax("outline.rs", source);
    let snapshot = buffer.snapshot();
    let item_start = source.find("Item").unwrap();
    let items = syntax
        .snapshot()
        .outline(0..snapshot.len_bytes().get(), &snapshot);

    let names: Vec<&str> = items.iter().map(|item| item.name.as_str()).collect();
    assert!(names.contains(&"数据"));
    assert!(names.iter().filter(|name| **name == "build").count() == 2);
    assert!(items.iter().any(|item| {
        item.name == "Item"
            && item.depth > 0
            && item.name_range == (item_start..item_start + "Item".len())
    }));
    assert!(items.iter().all(|item| item.version == snapshot.version()));
}

#[test]
fn outline_text_preserves_source_ranges_for_context_and_name() {
    let source = "pub struct DockData {\n    pub visible: bool,\n}\n";
    let (buffer, syntax) = parsed_syntax("outline.rs", source);
    let snapshot = buffer.snapshot();
    let item = syntax
        .snapshot()
        .outline(0..snapshot.len_bytes().get(), &snapshot)
        .into_iter()
        .find(|item| item.name == "DockData")
        .expect("结构体应出现在大纲中");

    assert!(item.text.contains("DockData"));
    assert!(
        item.text_ranges.iter().any(|part| {
            item.text[part.text_range.clone()] == source[part.source_range.clone()]
        })
    );
    assert!(item.text_ranges.iter().any(|part| {
        &item.text[part.text_range.clone()] == "DockData"
            && &source[part.source_range.clone()] == "DockData"
    }));
}

#[test]
fn outline_covers_markdown_and_html_injection_layers_in_source_coordinates() {
    let markdown = "# 文档\n\n```rust\nfn 初始化() {}\n```\n\n## 子节\n";
    let (buffer, syntax) = parsed_syntax("README.md", markdown);
    let snapshot = buffer.snapshot();
    let items = syntax
        .snapshot()
        .outline(0..snapshot.len_bytes().get(), &snapshot);
    assert!(
        items
            .iter()
            .any(|item| item.name == "文档" && item.language == "Markdown")
    );
    let function_start = markdown.find("初始化").unwrap();
    let function = items
        .iter()
        .find(|item| item.name == "初始化")
        .expect("围栏内 Rust 函数应出现在大纲中");
    assert_eq!(function.language, "Rust");
    assert_eq!(function.name_range.start, function_start);
    assert!(function.depth > 0);

    let html = "<main><section><h1>标题</h1></section></main>";
    let (buffer, syntax) = parsed_syntax("index.html", html);
    let snapshot = buffer.snapshot();
    let items = syntax
        .snapshot()
        .outline(0..snapshot.len_bytes().get(), &snapshot);
    let section = items
        .iter()
        .find(|item| item.name == "section")
        .expect("HTML 元素应出现在大纲中");
    assert_eq!(&html[section.name_range.clone()], "section");
    assert!(
        items
            .iter()
            .any(|item| item.name == "h1" && item.depth > section.depth)
    );
}

#[test]
fn outline_is_empty_without_declared_query() {
    let (buffer, syntax) = parsed_syntax("notes.txt", "标题\n");
    let snapshot = buffer.snapshot();
    assert!(
        syntax
            .snapshot()
            .outline(0..snapshot.len_bytes().get(), &snapshot)
            .is_empty()
    );
}

#[test]
fn rust_locals_resolve_unicode_references_and_shadowing() {
    let source = "fn 构建(值: i32) {\n    let 结果 = 值;\n    {\n        let 值 = 2;\n        let 内层 = 值;\n    }\n    let 最终 = 结果 + 值;\n}\n";
    let (buffer, syntax) = parsed_syntax("locals.rs", source);
    let snapshot = buffer.snapshot();
    let items = syntax
        .snapshot()
        .local_bindings(0..snapshot.len_bytes().get(), &snapshot);

    let value_definitions: Vec<_> = items.iter().filter(|item| item.name == "值").collect();
    assert_eq!(value_definitions.len(), 2);
    let parameter = value_definitions
        .iter()
        .find(|item| item.definition_range.start == source.find("值: i32").unwrap())
        .expect("参数绑定应存在");
    let shadowed = value_definitions
        .iter()
        .find(|item| item.definition_range.start > parameter.definition_range.start)
        .expect("嵌套作用域绑定应存在");
    assert!(parameter.references.iter().any(|range| {
        &source[range.clone()] == "值" && range.start > source.find("最终").unwrap()
    }));
    assert!(shadowed.references.iter().all(|range| {
        range.start > shadowed.definition_range.start && range.start < source.find("最终").unwrap()
    }));
    assert!(items.iter().all(|item| item.version == snapshot.version()));
}

#[test]
fn python_locals_resolve_parameters_and_nested_same_named_bindings() {
    let source = "def 构建(值):\n    结果 = 值\n    def 内层():\n        值 = 1\n        return 值\n    return 结果 + 值\n";
    let (buffer, syntax) = parsed_syntax("locals.py", source);
    let snapshot = buffer.snapshot();
    let items = syntax
        .snapshot()
        .local_bindings(0..snapshot.len_bytes().get(), &snapshot);

    let values: Vec<_> = items.iter().filter(|item| item.name == "值").collect();
    assert_eq!(values.len(), 2);
    let outer = values
        .iter()
        .find(|item| item.definition_range.start == source.find("值):").unwrap())
        .expect("Python 参数绑定应存在");
    let inner = values
        .iter()
        .find(|item| item.definition_range.start > outer.definition_range.start)
        .expect("嵌套函数绑定应存在");
    assert!(
        outer
            .references
            .iter()
            .any(|range| range.start > source.find("return 结果").unwrap())
    );
    assert!(inner.references.iter().any(|range| {
        range.start > inner.definition_range.start
            && range.start < source.find("return 结果").unwrap()
    }));
}

#[test]
fn javascript_locals_can_query_a_subrange_using_an_outer_parameter() {
    let source = "function 构建(值) {\n  const 结果 = 值;\n  return 结果;\n}\n";
    let (buffer, syntax) = parsed_syntax("locals.js", source);
    let snapshot = buffer.snapshot();
    let body_start = source.find("const").unwrap();
    let items = syntax
        .snapshot()
        .local_bindings(body_start..snapshot.len_bytes().get(), &snapshot);

    let parameter = items
        .iter()
        .find(|item| item.name == "值")
        .expect("子范围查询仍应返回外层参数");
    assert_eq!(&source[parameter.definition_range.clone()], "值");
    assert_eq!(parameter.references.len(), 1);
    assert_eq!(&source[parameter.references[0].clone()], "值");
    assert!(items.iter().any(|item| item.name == "结果"));
}

#[test]
fn go_and_c_family_locals_resolve_parameters_and_local_variables() {
    for (path, source, parameter, local) in [
        (
            "locals.go",
            "package demo\nfunc 构建(值 int) int { 结果 := 值; return 结果 }\n",
            "值",
            "结果",
        ),
        (
            "locals.c",
            "int 构建(int 值) { int 结果 = 值; return 结果; }\n",
            "值",
            "结果",
        ),
        (
            "locals.cpp",
            "int 构建(int 值) { int 结果 = 值; return 结果; }\n",
            "值",
            "结果",
        ),
    ] {
        let (buffer, syntax) = parsed_syntax(path, source);
        let snapshot = buffer.snapshot();
        let items = syntax
            .snapshot()
            .local_bindings(0..snapshot.len_bytes().get(), &snapshot);
        assert!(
            items.iter().any(|item| item.name == parameter),
            "{path} 参数绑定应存在"
        );
        assert!(
            items.iter().any(|item| item.name == local),
            "{path} 局部变量绑定应存在"
        );
        assert!(items.iter().all(|item| item.version == snapshot.version()));
    }
}

#[test]
fn local_bindings_reject_a_snapshot_from_another_buffer_version() {
    let (mut buffer, syntax) = rust_buffer("fn main(value: i32) { let result = value; }\n");
    let old_syntax = syntax.snapshot();
    let old_snapshot = buffer.snapshot();
    buffer
        .edit(
            [Edit::insert(ByteOffset::ZERO, "// 注释\n").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let new_snapshot = buffer.snapshot();

    assert_ne!(old_snapshot.version(), new_snapshot.version());
    assert!(
        old_syntax
            .local_bindings(0..new_snapshot.len_bytes().get(), &new_snapshot)
            .is_empty()
    );
}

#[test]
fn syntax_nodes_use_utf8_ranges_and_expand_within_one_layer() {
    let source = "fn 数据() {\n    let 值 = (1 + 2);\n}\n";
    let (buffer, syntax) = parsed_syntax("nodes.rs", source);
    let snapshot = buffer.snapshot();
    let syntax = syntax.snapshot();
    let name_start = source.find("值").unwrap();
    let node = syntax
        .node_at(name_start, &snapshot)
        .expect("Unicode 标识符应有语法节点");

    assert_eq!(&source[node.range.clone()], "值");
    assert_eq!(node.kind, "identifier");
    assert_eq!(node.language, "Rust");
    assert!(node.is_named);
    assert_eq!(node.version, snapshot.version());

    let ancestors = syntax.node_ancestors(name_start..name_start, &snapshot);
    assert_eq!(ancestors.first().map(|node| &node.range), Some(&node.range));
    assert!(ancestors.iter().any(|node| node.kind == "function_item"));

    let expanded = syntax
        .expand_selection_range(name_start..name_start, &snapshot)
        .expect("光标应能先扩展到标识符");
    assert_eq!(&source[expanded], "值");

    let anonymous = syntax
        .node_at(source.find('(').unwrap(), &snapshot)
        .expect("匿名括号节点应可导航");
    assert_eq!(anonymous.kind, "(");
    assert!(!anonymous.is_named);
    let whitespace_offset = source.find('\n').unwrap();
    let whitespace = syntax
        .node_at(whitespace_offset, &snapshot)
        .expect("空白位置应稳定落到包含它的语法根节点");
    assert!(whitespace.range.start <= whitespace_offset);
    assert!(whitespace_offset <= whitespace.range.end);
    assert!(syntax.node_at(source.len(), &snapshot).is_some());
}

#[test]
fn syntax_nodes_prefer_injection_layer_and_do_not_cross_back_to_host() {
    let source = "# 文档\n\n```rust\nfn 初始化() {}\n```\n";
    let (buffer, syntax) = parsed_syntax("README.md", source);
    let snapshot = buffer.snapshot();
    let syntax = syntax.snapshot();
    let name_start = source.find("初始化").unwrap();
    let node = syntax
        .node_at(name_start, &snapshot)
        .expect("围栏内函数名应有语法节点");

    assert_eq!(node.language, "Rust");
    assert_eq!(&source[node.range.clone()], "初始化");
    assert!(
        syntax
            .node_ancestors(name_start..name_start, &snapshot)
            .iter()
            .all(|node| node.language == "Rust")
    );
}

#[test]
fn syntax_nodes_keep_error_nodes_visible_for_incomplete_input() {
    let source = "fn main( {\n";
    let (buffer, syntax) = parsed_syntax("broken.rs", source);
    let snapshot = buffer.snapshot();
    let syntax = syntax.snapshot();
    let brace = source.find('{').unwrap();
    let ancestors = syntax.node_ancestors(brace..brace, &snapshot);

    assert!(ancestors.iter().any(|node| node.is_error));
}

#[test]
fn syntax_selection_reaches_file_root_for_import_and_structures() {
    let source = "use gpui::{\n    AnyElement,\n    AnyView,\n    App,\n};\n\nstruct EditorState {\n    value: usize,\n}\n";
    let (buffer, syntax) = parsed_syntax("selection.rs", source);
    let snapshot = buffer.snapshot();
    let syntax = syntax.snapshot();

    for needle in ["AnyElement", "EditorState"] {
        let mut range = source.find(needle).unwrap()..source.find(needle).unwrap();
        for _ in 0..16 {
            let Some(next) = syntax.expand_selection_range(range.clone(), &snapshot) else {
                break;
            };
            range = next;
        }
        assert_eq!(range, 0..source.len(), "{needle} 应能扩展到文件根节点");
    }
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
fn markdown_section_folds_through_nested_fenced_code() {
    let source = "# 第一节\n\n正文。\n\n```rust\nlet value = 1;\n```\n\n标题后的正文。\n\n# 第二节\n\n不应属于第一节。\n";
    let (buffer, syntax) = parsed_syntax("README.md", source);
    let snapshot = buffer.snapshot();
    let folds = syntax
        .snapshot()
        .fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
    let first_section = folds
        .iter()
        .find(|fold| fold.range.start == source.find('\n').unwrap())
        .expect("第一节应产生折叠范围");

    assert!(source[first_section.range.clone()].contains("标题后的正文。"));
    assert!(!source[first_section.range.clone()].contains("# 第二节"));
}

#[test]
fn structural_fold_ignores_nested_multiline_delimiters() {
    let source = "def build(\n    first,\n    second,\n):\n    return first + second\n";
    let (buffer, syntax) = parsed_syntax("build.py", source);
    let snapshot = buffer.snapshot();
    let folds = syntax
        .snapshot()
        .fold_ranges(0..snapshot.len_bytes().get(), &snapshot);

    assert!(
        folds
            .iter()
            .any(|fold| source[fold.range.clone()].contains("return first + second"))
    );
}

#[test]
fn macro_definition_declares_its_closing_boundary() {
    let source =
        "macro_rules! pair {\n    ($value:expr) => {\n        ($value, $value)\n    };\n}\n";
    let (buffer, syntax) = parsed_syntax("macros.rs", source);
    let snapshot = buffer.snapshot();
    let folds = syntax
        .snapshot()
        .fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
    let outer = folds
        .iter()
        .find(|fold| fold.range.start == source.find('\n').unwrap())
        .expect("宏定义应产生折叠范围");

    assert_eq!(outer.range.end, source.rfind('}').unwrap());
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
    let source = "struct Demo {\n    value: i32,\n}\n\nimpl Demo {\n    fn new() -> Self {\n        // 单行注释不产生折叠。\n        let value = 1;\n        // 连续注释折叠为一个组。\n        // 第二行注释。\n        Self { value }\n    }\n}\n\nfn main() {\n    let x = 1;\n}\n";
    let (buffer, syntax) = rust_buffer(source);
    let snapshot = buffer.snapshot();
    let syntax = syntax.snapshot();
    let folds = syntax.fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
    let texts: Vec<&str> = folds
        .iter()
        .map(|fold| &source[fold.range.clone()])
        .collect();

    assert!(texts.contains(&"\n    value: i32,\n"));
    assert!(texts.contains(&"\n        // 单行注释不产生折叠。\n        let value = 1;\n        // 连续注释折叠为一个组。\n        // 第二行注释。\n        Self { value }\n    "));
    assert!(texts.contains(&"\n        // 第二行注释。"));
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
    let source = "use std::collections::BTreeMap;\nuse std::ops::{\n    Range,\n    Deref,\n};\nuse std::sync::Arc;\n\nuse zcv_text::{\n    Buffer,\n    Snapshot,\n};\n";
    let (buffer, syntax) = rust_buffer(source);
    let snapshot = buffer.snapshot();
    let syntax = syntax.snapshot();
    let folds = syntax.fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
    let texts: Vec<&str> = folds
        .iter()
        .map(|fold| &source[fold.range.clone()])
        .collect();

    assert!(texts.contains(&"\n    Range,\n    Deref,\n"));
    assert!(texts.contains(&"\n    Buffer,\n    Snapshot,\n"));
    assert!(!texts.contains(&"use std::collections::BTreeMap;"));
}

#[test]
fn single_line_doc_comments_do_not_fold() {
    let source = "/// Editor 自身的领域事件。\n#[derive(Clone, Debug, PartialEq, Eq)]\npub enum EditorEvent {\n    /// 编辑器关联的文件路径发生变化。\n    PathChanged,\n}\n";
    let (buffer, syntax) = rust_buffer(source);
    let snapshot = buffer.snapshot();
    let syntax = syntax.snapshot();
    let folds = syntax.fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
    let texts: Vec<&str> = folds
        .iter()
        .map(|fold| &source[fold.range.clone()])
        .collect();

    assert!(texts.contains(&"\n    /// 编辑器关联的文件路径发生变化。\n    PathChanged,\n"));
    assert!(!texts.iter().any(|text| text.starts_with("///")));
}

#[test]
fn multi_line_macro_invocation_folds_but_single_line_does_not() {
    let source = "fn main() {\n    let x = vec![\n        1,\n        2,\n    ];\n    println!(\"ok\");\n    actions!(\n        editor,\n        [\n            MoveLeft,\n            MoveRight,\n        ],\n    );\n    let y = format!(\"{}: {}\", 1, 2);\n}\n";
    let (buffer, syntax) = rust_buffer(source);
    let snapshot = buffer.snapshot();
    let syntax = syntax.snapshot();
    let folds = syntax.fold_ranges(0..snapshot.len_bytes().get(), &snapshot);
    let texts: Vec<&str> = folds
        .iter()
        .map(|fold| &source[fold.range.clone()])
        .collect();

    assert!(texts.contains(&"\n        1,\n        2,\n    "));
    assert!(texts.contains(&"\n        editor,\n        [\n            MoveLeft,\n            MoveRight,\n        ],\n    "));
    assert!(!texts.contains(&"println!(\"ok\")"));
    assert!(!texts.contains(&"format!(\"{}: {}\", 1, 2)"));
}
