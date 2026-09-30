use super::*;
use zcv_language::OutlineItem;

fn item(text: &str, depth: usize, start: usize) -> OutlineItem {
    OutlineItem {
        version: Default::default(),
        range: start..start + 1,
        name_range: start..start + 1,
        name: text.to_string(),
        text: text.to_string(),
        text_ranges: Vec::new(),
        kind: "function".to_string(),
        depth,
        language: "Rust",
        language_depth: 0,
        body_range: None,
        annotation_range: None,
    }
}

fn entry(path: &str, text: &str, depth: usize, start: usize) -> OutlineEntry {
    OutlineEntry {
        display_path: std::path::PathBuf::from(path),
        item: item(text, depth, start),
    }
}

fn texts(rows: &[OutlineRow]) -> Vec<&str> {
    rows.iter()
        .filter_map(|row| match &row.kind {
            OutlineRowKind::Symbol(entry) => Some(entry.item.text.as_str()),
            _ => None,
        })
        .collect()
}

fn visible_texts(visible: &[(OutlineRow, bool, bool)]) -> Vec<&str> {
    visible
        .iter()
        .filter_map(|(row, _, _)| match &row.kind {
            OutlineRowKind::Symbol(entry) => Some(entry.item.text.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn collapsed_parent_hides_descendants_but_not_siblings() {
    let entries = vec![
        entry("src/a.rs", "mod", 0, 0),
        entry("src/a.rs", "fn", 1, 1),
        entry("src/a.rs", "fn2", 1, 2),
        entry("src/a.rs", "other", 0, 3),
    ];
    let rows = outline_rows(&entries, false, "", &HashSet::new());
    let collapsed: HashSet<_> = [rows[0].key()].into_iter().collect();
    let visible = outline_tree::visible_rows(&rows, &collapsed);
    assert_eq!(visible_texts(&visible), vec!["mod", "other"]);
    assert!(visible[0].1, "折叠父项应有子项");
    assert!(visible[0].2, "父项应标记为折叠");
}

#[test]
fn outline_refresh_requires_visible_panel_and_changed_version() {
    assert!(!outline_refresh_needed(false, true), "不可见面板不得重算");
    assert!(
        !outline_refresh_needed(true, false),
        "版本未变（滚动、重绘、选择变化）不得重算"
    );
    assert!(outline_refresh_needed(true, true), "版本变化且可见才重算");
}

#[test]
fn outline_filter_matches_text_case_insensitively() {
    let entries = vec![
        entry("src/a.rs", "fn build() {}", 0, 0),
        entry("src/a.rs", "struct 数据", 0, 1),
    ];
    let filtered = outline_rows(&entries, false, "build", &HashSet::new());
    assert_eq!(texts(&filtered), vec!["fn build() {}"]);
    assert_eq!(
        outline_rows(&entries, false, "", &HashSet::new()).len(),
        2,
        "空查询返回全部项"
    );
}

#[test]
fn has_children_follows_next_item_depth() {
    let entries = vec![
        entry("src/a.rs", "a", 0, 0),
        entry("src/a.rs", "b", 1, 1),
        entry("src/a.rs", "c", 0, 2),
    ];
    let rows = outline_rows(&entries, false, "", &HashSet::new());
    let visible = outline_tree::visible_rows(&rows, &HashSet::new());
    assert_eq!(visible.len(), 3);
    assert!(visible[0].1);
    assert!(!visible[1].1);
    assert!(!visible[2].1);
    assert!(visible.iter().all(|(_, _, collapsed)| !collapsed));
}

#[test]
fn multiple_files_build_directory_file_symbol_tree() {
    let entries = vec![
        entry("src/a.rs", "fn a", 0, 0),
        entry("src/nested/b.rs", "fn b", 0, 1),
    ];
    let rows = outline_rows(&entries, true, "", &HashSet::new());
    let structure: Vec<_> = rows
        .iter()
        .map(|row| match &row.kind {
            OutlineRowKind::Directory { name, .. } => format!("dir:{name}"),
            OutlineRowKind::File { name, .. } => format!("file:{name}"),
            OutlineRowKind::Symbol(entry) => format!("sym:{}", entry.item.text),
        })
        .collect();
    assert_eq!(
        structure,
        vec![
            "dir:src",
            "dir:nested",
            "file:b.rs",
            "sym:fn b",
            "file:a.rs",
            "sym:fn a",
        ]
    );

    // 折叠文件节点隐藏其符号，但保留同层其他节点。
    let file_key = rows
        .iter()
        .find(|row| matches!(row.kind, OutlineRowKind::File { .. }))
        .expect("文件节点应存在")
        .key();
    let collapsed: HashSet<_> = [file_key].into_iter().collect();
    let visible = outline_tree::visible_rows(&rows, &collapsed);
    let visible_structure: Vec<_> = visible
        .iter()
        .map(|(row, _, _)| match &row.kind {
            OutlineRowKind::Directory { name, .. } => format!("dir:{name}"),
            OutlineRowKind::File { name, .. } => format!("file:{name}"),
            OutlineRowKind::Symbol(entry) => format!("sym:{}", entry.item.text),
        })
        .collect();
    assert_eq!(
        visible_structure,
        vec![
            "dir:src",
            "dir:nested",
            "file:b.rs",
            "file:a.rs",
            "sym:fn a"
        ]
    );
}

#[test]
fn auto_fold_compresses_single_child_directory_chains() {
    let entries = vec![entry("a/b/c/file.rs", "fn x", 0, 0)];
    let rows = outline_rows(&entries, true, "", &HashSet::new());
    let structure: Vec<_> = rows
        .iter()
        .map(|row| match &row.kind {
            OutlineRowKind::Directory { name, .. } => format!("dir:{name}"),
            OutlineRowKind::File { name, .. } => format!("file:{name}"),
            OutlineRowKind::Symbol(entry) => format!("sym:{}", entry.item.text),
        })
        .collect();
    // a → b → c 是单子目录链，折叠成一行 a/b/c；c 含文件，是链条边界。
    assert_eq!(structure, vec!["dir:a/b/c", "file:file.rs", "sym:fn x"]);

    // 折叠这条链：行仍以最深目录为身份，子项隐藏。
    let directory_key = rows[0].key();
    let collapsed: HashSet<_> = [directory_key].into_iter().collect();
    let rebuilt = outline_rows(&entries, true, "", &collapsed);
    assert_eq!(rebuilt.len(), 1, "折叠的目录链不展开子项");
    let visible = outline_tree::visible_rows(&rebuilt, &collapsed);
    assert_eq!(visible.len(), 1);
    assert!(visible[0].2, "折叠链应标记为折叠");
}

#[test]
fn has_multiple_files_distinguishes_single_file_documents() {
    let single = vec![entry("src/a.rs", "a", 0, 0), entry("src/a.rs", "b", 1, 1)];
    assert!(!has_multiple_files(&single), "同一文件不构成文件树");
    let multiple = vec![entry("src/a.rs", "a", 0, 0), entry("src/b.rs", "b", 0, 1)];
    assert!(has_multiple_files(&multiple), "不同文件构成文件树");
}
