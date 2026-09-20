use std::time::Duration;

use zcv_text::{Buffer, BufferConfig, ByteOffset, Edit, TextRange, TransactionMetadata};

use super::*;
use crate::highlight_cache::HighlightCache;
use crate::test::{parsed_syntax, rust_buffer};

/// 测试共用：按给定编辑把语法映射推进到新版本（插值 + 后台解析 + 安装）。
fn edit_and_reparse(buffer: &mut Buffer, syntax: &mut SyntaxMap, edits: Vec<Edit>) {
    buffer.edit(edits, TransactionMetadata::default()).unwrap();
    let new_snapshot = buffer.snapshot();
    syntax.interpolate(&new_snapshot);
    let parsed = syntax
        .snapshot()
        .reparse(
            &new_snapshot,
            &syntax.registry(),
            &ParseCancellation::default(),
        )
        .expect("测试解析不应取消");
    assert!(syntax.did_parse(parsed));
}

/// 测试共用：按深度与语言名查找注入层（缺失时 panic）。
fn find_layer<'a>(layers: &'a [SyntaxLayer], depth: u32, language: &str) -> &'a SyntaxLayer {
    layers
        .iter()
        .find(|layer| layer.depth == depth && layer.language.name() == language)
        .unwrap_or_else(|| panic!("缺少 depth {depth} 的注入层 {language}"))
}

#[test]
fn go_annotated_string_creates_sql_injection_layer() {
    let source = "package main\nconst query = /* sql */ `SELECT name FROM users`\n";
    let (_, syntax) = parsed_syntax("main.go", source);
    let snapshot = syntax.snapshot();
    let sql = find_layer(snapshot.injection_layers(), 1, "SQL");

    assert_eq!(&source[sql.range.clone()], "SELECT name FROM users");
}

#[test]
fn unchanged_injection_layers_survive_sibling_edits_without_reparse() {
    // 编辑块 1 内容：块 2/3 的注入层与编辑前逐位相同（原样保留，不重新查询也不重新解析）。
    let source = "\
```rust
let a = 1;
```
```python
print(1)
```
```javascript
const x = 2;
```
";
    let (mut buffer, mut syntax) = parsed_syntax("README.md", source);
    assert_eq!(
        syntax.snapshot().injection_layers().len(),
        3,
        "三个围栏块各产生一个注入层"
    );

    // 变长编辑（"1" → "42"）：等长替换对 tree-sitter 增量解析不可见，无法用于断言"重新解析"。
    let one = source.find('1').unwrap();
    buffer
        .edit(
            [Edit::replace(
                TextRange::new(ByteOffset::new(one), ByteOffset::new(one + 1)).unwrap(),
                "42",
            )],
            TransactionMetadata::default(),
        )
        .unwrap();
    let new_snapshot = buffer.snapshot();
    syntax.interpolate(&new_snapshot);
    // 插值后的层：保留层此刻就是插值树本身（未重新解析）。
    let interpolated: Vec<SyntaxLayer> = syntax.snapshot().injection_layers().to_vec();
    let parsed = syntax
        .snapshot()
        .reparse(
            &new_snapshot,
            &syntax.registry(),
            &ParseCancellation::default(),
        )
        .expect("测试解析不应取消");
    assert!(syntax.did_parse(parsed));

    let snapshot = syntax.snapshot();
    let layers = snapshot.injection_layers();
    assert_eq!(layers.len(), 3, "层数量应保持不变");
    // 未受影响的注入层：插值树与最终树结构完全相同（原样保留，未重新解析；
    // 坐标同为编辑后版本，changed_ranges 是有效的结构比较）。
    let python_interpolated = find_layer(&interpolated, 1, "Python");
    let python = find_layer(layers, 1, "Python");
    assert_eq!(
        python
            .tree
            .changed_ranges(&python_interpolated.tree)
            .count(),
        0,
        "Python 层不应被重新解析"
    );
    let javascript_interpolated = find_layer(&interpolated, 1, "JavaScript");
    let javascript = find_layer(layers, 1, "JavaScript");
    assert_eq!(
        javascript
            .tree
            .changed_ranges(&javascript_interpolated.tree)
            .count(),
        0,
        "JavaScript 层不应被重新解析"
    );
    // 受影响的注入层：重新收集后范围跟随编辑后的文本坐标（"1" → "42" 使内容区终点 +1）。
    // 注意：等长或同构内容编辑不改变树结构，`changed_ranges` 对此不可见，用范围坐标断言。
    let rust = find_layer(layers, 1, "Rust");
    assert_eq!(rust.range, 8..20, "Rust 层范围应映射到编辑后的内容区");
}

#[test]
fn dbg3_ws() {
    let source = "```rust\nlet a = 1;\n```\n";
    let (mut buffer, mut syntax) = parsed_syntax("README.md", source);
    let fence = source.find("```rust").unwrap() + 3;
    buffer
        .edit(
            [Edit::insert(ByteOffset::new(fence + 4), " ").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let new_snapshot = buffer.snapshot();
    syntax.interpolate(&new_snapshot);
    let parsed = syntax
        .snapshot()
        .reparse(
            &new_snapshot,
            &syntax.registry(),
            &ParseCancellation::default(),
        )
        .expect("解析不应取消");
    assert!(syntax.did_parse(parsed));
}

#[test]
fn dbg2_fence() {
    let source = "```rust\nlet a = 1;\n```\n```python\nprint(1)\n```\n";
    let (mut buffer, mut syntax) = parsed_syntax("README.md", source);
    let rust = source.find("rust").unwrap();
    buffer
        .edit(
            [Edit::replace(
                TextRange::new(ByteOffset::new(rust), ByteOffset::new(rust + 4)).unwrap(),
                "python",
            )],
            TransactionMetadata::default(),
        )
        .unwrap();
    let new_snapshot = buffer.snapshot();
    syntax.interpolate(&new_snapshot);
    let parsed = syntax
        .snapshot()
        .reparse(
            &new_snapshot,
            &syntax.registry(),
            &ParseCancellation::default(),
        )
        .expect("解析不应取消");
    assert!(syntax.did_parse(parsed));
}

#[test]
fn fence_language_edit_replaces_the_injection_layer_without_duplicates() {
    // 围栏语言 rust → python：内容范围未变但注入语言变了。
    // 重新收集的注入必须替换保留的旧层；同深同内容范围只允许一层。
    let source = "```rust\nlet a = 1;\n```\n```python\nprint(1)\n```\n";
    let (mut buffer, mut syntax) = parsed_syntax("README.md", source);
    let rust = source.find("rust").unwrap();
    edit_and_reparse(
        &mut buffer,
        &mut syntax,
        vec![Edit::replace(
            TextRange::new(ByteOffset::new(rust), ByteOffset::new(rust + 4)).unwrap(),
            "python",
        )],
    );

    let snapshot = syntax.snapshot();
    let layers = snapshot.injection_layers();
    assert_eq!(
        layers
            .iter()
            .filter(|l| l.language.name() == "Rust")
            .count(),
        0,
        "围栏语言改为 python 后不应残留 Rust 层"
    );
    assert_eq!(
        layers
            .iter()
            .filter(|l| l.language.name() == "Python")
            .count(),
        2,
        "块 1 改为 python 后应有块 1 与块 2 两个 Python 层"
    );
    // 块 1 的内容范围（编辑后坐标）只被一个层覆盖。
    let text = buffer.snapshot();
    let content = text
        .slice_byte_range(ByteOffset::ZERO, text.len_bytes())
        .unwrap();
    let content_start = content.as_str().find("let a = 1;").unwrap();
    let covering = layers
        .iter()
        .filter(|l| l.range.start <= content_start && content_start < l.range.end)
        .count();
    assert_eq!(covering, 1, "块 1 内容只应被一个注入层覆盖");
}

#[test]
fn whitespace_fence_edit_keeps_the_layer_without_duplicates() {
    // "```rust" → "``` rust"：语言名 trim 后相同，保留层直接复用，不产生重复层。
    let source = "```rust\nlet a = 1;\n```\n";
    let (mut buffer, mut syntax) = parsed_syntax("README.md", source);
    let fence = source.find("```rust").unwrap() + 3;
    buffer
        .edit(
            [Edit::insert(ByteOffset::new(fence + 4), " ").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let new_snapshot = buffer.snapshot();
    syntax.interpolate(&new_snapshot);
    let interpolated = syntax.snapshot().injection_layers().to_vec();
    let parsed = syntax
        .snapshot()
        .reparse(
            &new_snapshot,
            &syntax.registry(),
            &ParseCancellation::default(),
        )
        .expect("测试解析不应取消");
    assert!(syntax.did_parse(parsed));

    let snapshot = syntax.snapshot();
    let layers = snapshot.injection_layers();
    let rust_layers: Vec<_> = layers
        .iter()
        .filter(|l| l.language.name() == "Rust")
        .collect();
    assert_eq!(rust_layers.len(), 1, "语言名未变不应产生重复层");
    // 保留层：插值树与最终树结构完全相同（未重新解析、未重复收集）。
    let interpolated_rust = find_layer(&interpolated, 1, "Rust");
    assert_eq!(
        rust_layers[0]
            .tree
            .changed_ranges(&interpolated_rust.tree)
            .count(),
        0,
        "内容未变时注入树应原样保留"
    );
}

#[test]
fn added_and_removed_fenced_blocks_update_layers_incrementally() {
    let source = "```rust\nlet a = 1;\n```\n```python\nprint(1)\n```\n";
    let (mut buffer, mut syntax) = parsed_syntax("README.md", source);
    let end = buffer.len_bytes();

    // 追加新围栏块 → 新增注入层。
    edit_and_reparse(
        &mut buffer,
        &mut syntax,
        vec![Edit::insert(end, "```javascript\nconst x = 2;\n```\n").unwrap()],
    );
    let snapshot = syntax.snapshot();
    let layers = snapshot.injection_layers();
    assert_eq!(layers.len(), 3, "追加围栏块后应新增一层");
    assert!(
        layers.iter().any(|l| l.language.name() == "JavaScript"),
        "新增层应为 JavaScript"
    );

    // 删除中间的 python 围栏块 → 对应注入层消失，其余保留。
    let text = buffer.snapshot();
    let all = text
        .slice_byte_range(ByteOffset::ZERO, text.len_bytes())
        .unwrap();
    let python_start = all.as_str().find("```python").unwrap();
    let python_end = all.as_str().find("```\n```javascript").unwrap() + 3;
    edit_and_reparse(
        &mut buffer,
        &mut syntax,
        vec![Edit::replace(
            TextRange::new(ByteOffset::new(python_start), ByteOffset::new(python_end)).unwrap(),
            String::new(),
        )],
    );
    let snapshot = syntax.snapshot();
    let layers = snapshot.injection_layers();
    assert_eq!(layers.len(), 2, "删除围栏块后应回到两层");
    assert!(
        layers.iter().all(|l| l.language.name() != "Python"),
        "Python 注入层应随围栏块删除而消失"
    );
}

#[test]
fn nested_injection_recollects_only_within_inner_changed_ranges() {
    // 围栏 markdown 块内的 inline 注入（depth 2）：
    // 编辑内层段落，嵌套层经递归按内层树的变化区间重收集，兄弟注入层不受影响。
    let source = "```markdown\nHello *world*\n```\n```rust\nlet a = 1;\n```\n";
    let (mut buffer, mut syntax) = parsed_syntax("README.md", source);
    let layers_before = syntax.snapshot().injection_layers().to_vec();
    assert_eq!(
        layers_before.len(),
        3,
        "外层 markdown + rust + 内层 inline 共三层"
    );
    find_layer(&layers_before, 2, "Markdown Inline");

    let world = source.find("world").unwrap();
    buffer
        .edit(
            [Edit::replace(
                TextRange::new(ByteOffset::new(world), ByteOffset::new(world + 5)).unwrap(),
                "planets",
            )],
            TransactionMetadata::default(),
        )
        .unwrap();
    let new_snapshot = buffer.snapshot();
    syntax.interpolate(&new_snapshot);
    let interpolated = syntax.snapshot().injection_layers().to_vec();
    let parsed = syntax
        .snapshot()
        .reparse(
            &new_snapshot,
            &syntax.registry(),
            &ParseCancellation::default(),
        )
        .expect("测试解析不应取消");
    assert!(syntax.did_parse(parsed));

    let snapshot = syntax.snapshot();
    let layers = snapshot.injection_layers();
    assert_eq!(layers.len(), 3, "层数量应保持不变");
    // 内层 inline 注入重新收集后范围覆盖编辑后的新文本（"world" → "planets" +2 字节）。
    // 内容编辑不改变树结构，`changed_ranges` 对此不可见，用范围坐标断言。
    let inline = find_layer(layers, 2, "Markdown Inline");
    let text = buffer.snapshot();
    let planets = text
        .slice_byte_range(ByteOffset::ZERO, text.len_bytes())
        .unwrap()
        .as_str()
        .find("planets")
        .expect("编辑后的文本应包含 planets");
    assert!(
        inline.range.start <= planets && planets < inline.range.end,
        "内层 inline 注入范围应覆盖编辑后的新文本"
    );
    let rust = find_layer(layers, 1, "Rust");
    let rust_interpolated = find_layer(&interpolated, 1, "Rust");
    assert_eq!(
        rust.tree.changed_ranges(&rust_interpolated.tree).count(),
        0,
        "兄弟注入层应原样保留"
    );
}

#[test]
fn layers_for_range_queries_across_depths_and_points() {
    // 按 (深度, 起点) 有序 + 同深不交：范围查询覆盖跨深度命中、
    // 空查询（点包含）、以及起点恰在查询起点/终点的边界。
    let source = "\
```markdown
Hello *world*
```
```rust
let a = 1;
```
```rust
let b = 2;
```
";
    let (buffer, syntax) = parsed_syntax("README.md", source);
    let snapshot = buffer.snapshot();
    let syntax = syntax.snapshot();
    let all = snapshot
        .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
        .unwrap();
    let full = 0..snapshot.len_bytes().get();

    // 全文查询：外层 markdown 层 + 内层 inline + 两个 Rust 层全部命中。
    let names: Vec<&str> = syntax
        .layers_for_range(&full)
        .map(|layer| layer.language.name())
        .collect();
    assert!(names.contains(&"Markdown"));
    assert!(names.contains(&"Markdown Inline"));
    assert_eq!(names.iter().filter(|name| **name == "Rust").count(), 2);

    // 空查询（光标点）：命中的层必须包含该点。
    let world = all.as_str().find("*world*").unwrap() + 1;
    let point_hits: Vec<_> = syntax
        .layers_for_range(&(world..world))
        .map(|layer| layer.language.name())
        .collect();
    assert!(point_hits.contains(&"Markdown"), "外层层应包含光标点");
    assert!(
        point_hits.contains(&"Markdown Inline"),
        "内层 inline 应包含光标点"
    );

    // 只查询第二个 Rust 块：第一个 Rust 层不得命中。
    let second_rust = all.as_str().rfind("let b = 2;").unwrap();
    let first_rust = all.as_str().find("let a = 1;").unwrap();
    let rust_hits: Vec<_> = syntax
        .layers_for_range(&(first_rust..second_rust + 3))
        .map(|layer| layer.language.name())
        .collect();
    assert_eq!(
        rust_hits.iter().filter(|name| **name == "Rust").count(),
        2,
        "区间覆盖两个 Rust 块时应都命中"
    );
    let single_rust: Vec<_> = syntax
        .layers_for_range(&(second_rust..second_rust + 1))
        .map(|layer| layer.language.name())
        .collect();
    assert_eq!(
        single_rust.iter().filter(|name| **name == "Rust").count(),
        1,
        "只落在第二个块内的区间不应命中第一个 Rust 层"
    );
}

#[test]
fn syntax_ancestor_uses_the_smallest_layer() {
    let source = "fn main() { let value = 1; }\n";
    let (buffer, syntax) = rust_buffer(source);
    let snapshot = buffer.snapshot();
    let syntax = syntax.snapshot();
    let caret = source.find("value").unwrap();
    let identifier = syntax
        .expand_selection_range(caret..caret, &snapshot)
        .expect("光标应扩展到 identifier");
    assert_eq!(&source[identifier.clone()], "value");
    let parent = syntax
        .expand_selection_range(identifier, &snapshot)
        .expect("identifier 应继续扩展到父语法节点");
    assert!(parent.len() > "value".len());
}

#[test]
fn syntax_snapshots_share_immutable_state_until_interpolation() {
    let (mut buffer, mut syntax) = rust_buffer("fn main() {}\n");
    let first = syntax.snapshot();
    let second = syntax.snapshot();
    assert!(Arc::ptr_eq(&first.state, &second.state));

    let old_snapshot = buffer.snapshot();
    buffer
        .edit(
            [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let new_snapshot = buffer.snapshot();
    syntax.interpolate(&new_snapshot);

    let interpolated = syntax.snapshot();
    assert!(!Arc::ptr_eq(&first.state, &interpolated.state));
    assert_eq!(first.version(), old_snapshot.version());
    assert_eq!(interpolated.version(), new_snapshot.version());
}

#[test]
fn time_sliced_parse_resumes_and_matches_single_pass_result() {
    // 极短预算（1ns）强制多次中断-恢复：分片完成的树必须与一次性解析逐位等价。
    // 小文件可能在首个 progress callback 前就完成，用 ~500 行代码确保多次分片。
    let mut source = String::new();
    for index in 0..500 {
        source.push_str(&format!(
            "fn function_{index}(value: i32) -> i32 {{\n    let result = value * {index};\n    result\n}}\n\n"
        ));
    }
    let buffer = Buffer::from_text(source.to_owned(), BufferConfig::default()).unwrap();
    let snapshot = buffer.snapshot();
    let language = Arc::new(LanguageRegistry::new())
        .language_for_file(Path::new("main.rs"), None)
        .expect("Rust 语言应可加载");
    let cancellation = ParseCancellation::default();

    // 一次性解析（对照）。
    let single =
        parse_tree(&language, &snapshot, None, None, &cancellation).expect("一次性解析应成功");

    // 分片解析：逐片推进直到完成。
    let mut parser = IncrementalParser::new();
    let mut slices = 0usize;
    let sliced = loop {
        slices += 1;
        assert!(slices < 10_000, "分片解析应在有限片数内完成（预算过小？）");
        match parser.parse_slice(
            &language,
            &snapshot,
            None,
            None,
            &cancellation,
            Duration::from_nanos(1),
        ) {
            Some(tree) => break tree,
            None => {
                assert!(!cancellation.is_cancelled());
                continue;
            }
        }
    };
    assert!(slices > 1, "1ns 预算应产生多次分片，实际 {slices} 片");
    assert_eq!(
        single.changed_ranges(&sliced).count(),
        0,
        "分片恢复解析应与一次性解析结构等价"
    );
}

#[test]
fn time_sliced_parse_aborts_on_cancellation() {
    let source = "fn main() { let value = 1; }\n";
    let buffer = Buffer::from_text(source.to_owned(), BufferConfig::default()).unwrap();
    let snapshot = buffer.snapshot();
    let language = Arc::new(LanguageRegistry::new())
        .language_for_file(Path::new("main.rs"), None)
        .expect("Rust 语言应可加载");
    let cancellation = ParseCancellation::default();
    let mut parser = IncrementalParser::new();
    // 首片后取消：后续片必须放弃，不产生死循环。
    let _ = parser.parse_slice(
        &language,
        &snapshot,
        None,
        None,
        &cancellation,
        Duration::from_nanos(1),
    );
    cancellation.cancel();
    assert!(
        parser
            .parse_slice(
                &language,
                &snapshot,
                None,
                None,
                &cancellation,
                Duration::from_nanos(1),
            )
            .is_none()
    );
}

#[test]
fn cancelled_parse_produces_no_installable_snapshot() {
    let buffer = Buffer::from_text("fn main() {}\n".to_owned(), Default::default()).unwrap();
    let snapshot = buffer.snapshot();
    let mut syntax = SyntaxMap::new(Arc::new(LanguageRegistry::new()), &snapshot);
    let first_line = "fn main() {}";
    syntax.set_language_for_file(Path::new("main.rs"), Some(first_line), &snapshot);
    let cancellation = ParseCancellation::default();
    cancellation.cancel();

    assert!(
        syntax
            .snapshot()
            .reparse(&snapshot, &syntax.registry(), &cancellation)
            .is_none()
    );
}

#[test]
fn unchanged_injection_reuses_its_tree_across_parent_edits() {
    let source = "<style>.item { color: red; }</style><script>let value = 1;</script>";
    let (mut buffer, mut syntax) = parsed_syntax("index.html", source);
    let red = source.find("red").unwrap();
    buffer
        .edit(
            [Edit::replace(
                TextRange::new(ByteOffset::new(red), ByteOffset::new(red + 3)).unwrap(),
                "blue",
            )],
            TransactionMetadata::default(),
        )
        .unwrap();
    let new_snapshot = buffer.snapshot();
    syntax.interpolate(&new_snapshot);
    let interpolated_tree = syntax
        .snapshot()
        .injection_layers()
        .iter()
        .find(|layer| layer.language.name() == "JavaScript")
        .expect("HTML 应包含 JavaScript 注入层")
        .tree
        .clone();
    let parsed = syntax
        .snapshot()
        .reparse(
            &new_snapshot,
            &syntax.registry(),
            &ParseCancellation::default(),
        )
        .expect("测试解析不应取消");
    assert!(syntax.did_parse(parsed));

    let parsed_tree = syntax
        .snapshot()
        .injection_layers()
        .iter()
        .find(|layer| layer.language.name() == "JavaScript")
        .expect("编辑 CSS 后 JavaScript 注入层应保留")
        .tree
        .clone();
    assert_eq!(interpolated_tree.changed_ranges(&parsed_tree).count(), 0);
}

#[test]
fn incrementally_reparses_after_edit() {
    let (mut buffer, mut syntax) = rust_buffer("fn main() { let value = 1; }\n");
    let old_snapshot = buffer.snapshot();
    let start = old_snapshot
        .slice_byte_range(ByteOffset::ZERO, old_snapshot.len_bytes())
        .unwrap()
        .as_str()
        .find('1')
        .unwrap();
    buffer
        .edit(
            [Edit::insert(ByteOffset::new(start), "\"文本\"").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let new_snapshot = buffer.snapshot();
    syntax.interpolate(&new_snapshot);
    let parsed = syntax
        .snapshot()
        .reparse(
            &new_snapshot,
            &syntax.registry(),
            &ParseCancellation::default(),
        )
        .expect("测试解析不应取消");
    assert!(syntax.did_parse(parsed));

    let syntax_snapshot = syntax.snapshot();
    let names = syntax_snapshot.capture_names();
    let spans = syntax_snapshot.highlights(
        0..new_snapshot.len_bytes().get(),
        &new_snapshot,
        &HighlightCache::new(),
    );
    assert!(
        spans
            .iter()
            .any(|span| names[span.capture as usize].as_ref() == "string")
    );
    assert_eq!(syntax.snapshot().version(), new_snapshot.version());
}

#[test]
fn incrementally_reparses_multiple_unicode_edits() {
    let (mut buffer, mut syntax) = rust_buffer("fn main() { let x = 1; let y = 2; }\n");
    let old_snapshot = buffer.snapshot();
    let source = old_snapshot
        .slice_byte_range(ByteOffset::ZERO, old_snapshot.len_bytes())
        .unwrap();
    let first = source.as_str().find('1').unwrap();
    let second = source.as_str().find('2').unwrap();
    buffer
        .edit(
            [
                Edit::replace(
                    TextRange::new(ByteOffset::new(first), ByteOffset::new(first + 1)).unwrap(),
                    "\"一\"",
                ),
                Edit::replace(
                    TextRange::new(ByteOffset::new(second), ByteOffset::new(second + 1)).unwrap(),
                    "\"二\"",
                ),
            ],
            TransactionMetadata::default(),
        )
        .unwrap();
    let new_snapshot = buffer.snapshot();
    syntax.interpolate(&new_snapshot);
    let parsed = syntax
        .snapshot()
        .reparse(
            &new_snapshot,
            &syntax.registry(),
            &ParseCancellation::default(),
        )
        .expect("测试解析不应取消");
    assert!(syntax.did_parse(parsed));

    let syntax_snapshot = syntax.snapshot();
    let names = syntax_snapshot.capture_names();
    let string_count = syntax_snapshot
        .highlights(
            0..new_snapshot.len_bytes().get(),
            &new_snapshot,
            &HighlightCache::new(),
        )
        .iter()
        .filter(|span| names[span.capture as usize].as_ref() == "string")
        .count();
    assert_eq!(string_count, 2);
}

#[test]
fn stale_parse_result_cannot_replace_interpolated_tree() {
    let (mut buffer, mut syntax) = rust_buffer("fn main() {}\n");
    let stale_parse = syntax.snapshot();
    let old_snapshot = buffer.snapshot();
    buffer
        .edit(
            [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let new_snapshot = buffer.snapshot();
    syntax.interpolate(&new_snapshot);

    let stale = stale_parse
        .reparse(
            &old_snapshot,
            &syntax.registry(),
            &ParseCancellation::default(),
        )
        .expect("测试解析不应取消");
    assert!(!syntax.did_parse(stale));
    assert_eq!(syntax.snapshot().version(), new_snapshot.version());
}
