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
fn sanitize_selection_drops_descendants_sorts_and_excludes_root() {
    let root = abs("/proj");
    let dir = abs(root.join("src"));
    let file = abs(dir.join("main.rs"));
    let sibling = abs(root.join("a.txt"));
    // 乱序输入：目录与其子文件同选只留目录，根被剔除，输出按路径排序。
    assert_eq!(
        sanitize_selection(
            [file.clone(), sibling.clone(), dir.clone(), root.clone()],
            &root
        ),
        vec![sibling, dir]
    );
}

#[test]
fn sanitize_selection_excludes_paths_outside_project() {
    let root = abs("/proj");
    let outside = abs("/other/file.txt");
    let inside = abs(root.join("b.txt"));
    assert_eq!(
        sanitize_selection([root.clone(), outside, inside.clone()], &root),
        vec![inside]
    );
}

#[test]
fn sanitize_selection_treats_sibling_prefix_as_non_ancestor() {
    let root = abs("/proj");
    let a = abs(root.join("a"));
    let ab = abs(root.join("ab"));
    // 组件级比较：a 与 ab 互不为祖先，两条都保留。
    assert_eq!(
        sanitize_selection([ab.clone(), a.clone()], &root),
        vec![a, ab]
    );
}

#[test]
fn cut_clipboard_degrades_to_copied_after_paste() {
    let paths = vec![abs("/proj/a.txt")];
    let clipboard = TreeClipboard::Cut(paths.clone()).into_copied();
    assert!(matches!(clipboard, TreeClipboard::Copied(ref degraded) if degraded == &paths));
    // 已是复制：原样返回，不改动。
    let copied = TreeClipboard::Copied(paths.clone()).into_copied();
    assert!(matches!(copied, TreeClipboard::Copied(ref kept) if kept == &paths));
}

#[test]
fn clipboard_paths_accessor_covers_both_variants() {
    let paths = vec![abs("/proj/a.txt")];
    assert_eq!(
        TreeClipboard::Copied(paths.clone()).paths(),
        paths.as_slice()
    );
    assert_eq!(TreeClipboard::Cut(paths.clone()).paths(), paths.as_slice());
}

/// 构造三项冲突会话：源/目标对按序可分辨。
fn session(mode: TransferMode) -> ConflictSession {
    ConflictSession::new(
        mode,
        abs("/proj/dst"),
        vec![
            (abs("/proj/a.txt"), abs("/proj/dst/a.txt")),
            (abs("/proj/b.txt"), abs("/proj/dst/b.txt")),
            (abs("/proj/c.txt"), abs("/proj/dst/c.txt")),
        ],
    )
}

#[test]
fn session_with_all_overwrite_decisions_resolves_in_order() {
    let mut session = session(TransferMode::Copy);
    assert_eq!(session.len(), 3);
    assert!(!session.is_empty());
    assert_eq!(
        session.current_conflict().map(|(source, _)| source),
        Some(&abs("/proj/a.txt"))
    );

    session.record_decision(ConflictDecision::Overwrite);
    assert_eq!(
        session.current_conflict().map(|(source, _)| source),
        Some(&abs("/proj/b.txt"))
    );
    assert!(!session.is_resolved());

    session.record_decision(ConflictDecision::Overwrite);
    session.record_decision(ConflictDecision::Overwrite);
    assert!(session.is_resolved());
    assert_eq!(session.current_conflict(), None);
    assert_eq!(
        session.decisions(),
        &[ConflictDecision::Overwrite; 3],
        "全部覆盖：三项决策按序记录"
    );
}

#[test]
fn session_with_all_skip_decisions_resolves() {
    let mut session = session(TransferMode::Move);
    for _ in 0..3 {
        session.record_decision(ConflictDecision::Skip);
    }
    assert!(session.is_resolved());
    assert_eq!(session.decisions(), &[ConflictDecision::Skip; 3]);
}

#[test]
fn session_with_mixed_decisions_preserves_order() {
    let mut session = session(TransferMode::Copy);
    session.record_decision(ConflictDecision::Overwrite);
    session.record_decision(ConflictDecision::Skip);
    session.record_decision(ConflictDecision::Overwrite);
    assert!(session.is_resolved());
    assert_eq!(
        session.decisions(),
        &[
            ConflictDecision::Overwrite,
            ConflictDecision::Skip,
            ConflictDecision::Overwrite
        ]
    );
}

#[test]
fn empty_session_is_immediately_resolved() {
    let session = ConflictSession::new(TransferMode::Copy, abs("/proj/dst"), Vec::new());
    assert!(session.is_empty());
    assert_eq!(session.len(), 0);
    assert!(session.is_resolved(), "空会话无待决策项，视为已解决");
    assert_eq!(session.current_conflict(), None);
}

#[test]
fn paste_target_dir_follows_selection_kind() {
    let root = abs("/proj");
    // 目录 → 自身。
    assert_eq!(
        paste_target_dir(Some(&abs(root.join("src"))), true),
        Some(abs(root.join("src")))
    );
    // 文件 → 父目录。
    assert_eq!(
        paste_target_dir(Some(&abs(root.join("src").join("main.rs"))), false),
        Some(abs(root.join("src")))
    );
    // 根级文件的父目录即项目根。
    assert_eq!(
        paste_target_dir(Some(&abs(root.join("a.txt"))), false),
        Some(root.clone())
    );
    // 无选中 → None。
    assert_eq!(paste_target_dir(None, true), None);
}
