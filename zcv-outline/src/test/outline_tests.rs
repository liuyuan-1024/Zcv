use super::*;

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

#[test]
fn collapsed_parent_hides_descendants_but_not_siblings() {
    let items = vec![
        item("mod", 0, 0),
        item("fn", 1, 1),
        item("fn2", 1, 2),
        item("other", 0, 3),
    ];
    let collapsed: HashSet<_> = [OutlineItemKey::from_item(&items[0])].into_iter().collect();
    let visible = visible_items(&items, &collapsed);
    let texts: Vec<_> = visible
        .iter()
        .map(|(item, _, _)| item.text.as_str())
        .collect();
    assert_eq!(texts, vec!["mod", "other"]);
    assert!(visible[0].1, "折叠父项应有子项");
    assert!(visible[0].2, "父项应标记为折叠");
}

#[test]
fn has_children_follows_next_item_depth() {
    let items = vec![item("a", 0, 0), item("b", 1, 1), item("c", 0, 2)];
    let visible = visible_items(&items, &HashSet::new());
    assert_eq!(visible.len(), 3);
    assert!(visible[0].1);
    assert!(!visible[1].1);
    assert!(!visible[2].1);
    assert!(visible.iter().all(|(_, _, collapsed)| !collapsed));
}
