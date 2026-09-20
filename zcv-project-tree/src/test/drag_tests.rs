use std::path::PathBuf;

use super::*;

fn abs(path: impl Into<PathBuf>) -> AbsolutePathBuf {
    let path = path.into();
    let path = if path.is_absolute() {
        path
    } else {
        let relative = path
            .to_string_lossy()
            .trim_start_matches(&['/', '\\'][..])
            .to_owned();
        std::env::current_dir()
            .expect("测试应能取得当前目录")
            .join(relative)
    };
    AbsolutePathBuf::new(path).expect("测试树路径必须是绝对路径")
}

#[test]
fn items_follows_active_membership_in_marked_snapshot() {
    let root = abs("/proj");
    let a = abs(root.join("a.txt"));
    let b = abs(root.join("b.txt"));
    // 被拖行在标记集内：整个标记集参与拖拽。
    let drag = TreeDrag {
        active_selection: a.clone(),
        marked_selections: vec![a.clone(), b.clone()].into(),
        preview_name: "a.txt".into(),
    };
    assert_eq!(drag.items(), vec![a.clone(), b.clone()]);
    // 被拖行不在标记集内（集合外行拖起）：仅被拖行参与。
    let drag = TreeDrag {
        active_selection: a.clone(),
        marked_selections: vec![b.clone()].into(),
        preview_name: "a.txt".into(),
    };
    assert_eq!(drag.items(), vec![a]);
}

#[test]
fn drop_target_of_directory_row_is_itself() {
    let root = abs("/proj");
    // 目录行 → 自身路径。
    assert_eq!(
        drop_target_dir(&abs(root.join("src")), true),
        Some(abs(root.join("src")))
    );
    // 根行（项目根目录行）→ 项目根。
    assert_eq!(drop_target_dir(&root, true), Some(root.clone()));
}

#[test]
fn drop_target_of_file_row_is_its_parent_directory() {
    let root = abs("/proj");
    // 普通文件行 → 父目录。
    assert_eq!(
        drop_target_dir(&abs(root.join("src").join("main.rs")), false),
        Some(abs(root.join("src")))
    );
    // 根级文件行 → 项目根。
    assert_eq!(
        drop_target_dir(&abs(root.join("a.txt")), false),
        Some(root.clone())
    );
}

#[test]
fn moving_directory_into_own_subtree_is_rejected() {
    let root = abs("/proj");
    let src = abs(root.join("src"));
    let sub = abs(src.join("sub"));
    // 目标是源的后代：拒绝。
    assert_eq!(
        filter_movable_sources(std::slice::from_ref(&src), &sub),
        Vec::<AbsolutePathBuf>::new()
    );
    // 目标是源本身（目录拖回自己身上）：拒绝。
    assert_eq!(
        filter_movable_sources(std::slice::from_ref(&src), &src),
        Vec::<AbsolutePathBuf>::new()
    );
}

#[test]
fn moving_into_own_ancestor_directory_is_rejected() {
    let root = abs("/proj");
    // 移动结果由「落点目录 + 源的最后组件名」拼出：源 /proj/a/b/a/c/a 拖到 /proj/a/b 上，
    // 结果 /proj/a/b/a 恰好是源的祖先目录，覆盖移动会摧毁源数据，必须拒绝（与 move_path 的对称守卫一致）。
    let source = abs(root.join("a").join("b").join("a").join("c").join("a"));
    let ancestor_target = abs(root.join("a").join("b"));
    assert_eq!(
        filter_movable_sources(std::slice::from_ref(&source), &ancestor_target),
        Vec::<AbsolutePathBuf>::new()
    );
    // 落点是无关的兄弟目录时照常放行。
    let sibling = abs(root.join("other"));
    assert_eq!(
        filter_movable_sources(std::slice::from_ref(&source), &sibling),
        vec![source]
    );
}

#[test]
fn moving_into_same_directory_is_filtered_as_noop() {
    let root = abs("/proj");
    let file = abs(root.join("a.txt"));
    // 落回源所在目录：目标与源相同，剔除。
    assert_eq!(
        filter_movable_sources(std::slice::from_ref(&file), &root),
        Vec::<AbsolutePathBuf>::new()
    );
}

#[test]
fn moving_into_sibling_directory_is_allowed() {
    let root = abs("/proj");
    let file = abs(root.join("a.txt"));
    let dst = abs(root.join("dst"));
    assert_eq!(
        filter_movable_sources(std::slice::from_ref(&file), &dst),
        vec![file]
    );
}

#[test]
fn multi_select_drag_filters_each_source_independently() {
    let root = abs("/proj");
    let src = abs(root.join("src"));
    let inner = abs(src.join("inner.txt"));
    let other = abs(root.join("other.txt"));
    // 多选拖到 src/自身：目录源因「移入自身子树」被拒，
    // src 内文件因「落回原目录」被剔除，项目外文件正常放行。
    let kept = filter_movable_sources(&[src.clone(), inner, other.clone()], &src);
    assert_eq!(kept, vec![other]);
}

#[test]
fn project_root_row_is_not_movable() {
    let root = abs("/proj");
    // 根路径无文件名，视为不可移动。
    assert_eq!(
        filter_movable_sources(std::slice::from_ref(&root), &abs(root.join("dst"))),
        Vec::<AbsolutePathBuf>::new()
    );
}
