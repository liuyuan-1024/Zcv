use std::path::Path;

use zcv_path::AbsolutePathBuf;

use super::{ProjectEntry, canonical_project_path, first_valid_project};

#[test]
fn recent_project_rejects_root_relative_and_missing_paths() {
    assert!(canonical_project_path(Path::new("/")).is_none());
    assert!(canonical_project_path(Path::new(".")).is_none());
    assert!(canonical_project_path(Path::new("/definitely/missing/zcv-project")).is_none());
}

#[test]
fn recent_project_canonicalizes_parent_components() {
    let current = AbsolutePathBuf::canonicalize(&std::env::current_dir().expect("应有当前目录"))
        .expect("当前目录应可规范化")
        .into_path_buf();
    let with_parent = current
        .join("..")
        .join(current.file_name().expect("当前目录应有名称"));
    assert_eq!(canonical_project_path(&with_parent), Some(current));
}

#[test]
fn first_valid_project_skips_stale_entries() {
    let current = std::env::current_dir().expect("应有当前目录");
    let entries = vec![
        ProjectEntry {
            path: "/definitely/missing/zcv-project".into(),
        },
        ProjectEntry {
            path: current.to_string_lossy().to_string(),
        },
    ];
    assert_eq!(first_valid_project(&entries), Some(current));
}

#[test]
fn first_valid_project_returns_none_when_all_stale() {
    let entries = vec![
        ProjectEntry {
            path: "/definitely/missing/a".into(),
        },
        ProjectEntry {
            path: "/definitely/missing/b".into(),
        },
    ];
    assert_eq!(first_valid_project(&entries), None);
}
