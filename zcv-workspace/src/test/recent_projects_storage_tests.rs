use std::{env, path::Path};

use zcv_path::AbsolutePathBuf;

use super::{ProjectEntry, canonical_project_path, valid_project_paths};

#[test]
fn recent_project_rejects_root_relative_and_missing_paths() {
    assert!(canonical_project_path(Path::new("/")).is_none());
    assert!(canonical_project_path(Path::new(".")).is_none());
    assert!(canonical_project_path(Path::new("/definitely/missing/zcv-project")).is_none());
}

#[test]
fn recent_project_canonicalizes_parent_components() {
    let current = AbsolutePathBuf::canonicalize(&env::current_dir().expect("应有当前目录"))
        .expect("当前目录应可规范化")
        .into_path_buf();
    let with_parent = current
        .join("..")
        .join(current.file_name().expect("当前目录应有名称"));
    assert_eq!(canonical_project_path(&with_parent), Some(current));
}

#[test]
fn valid_project_paths_keeps_valid_subset_in_order() {
    let current = env::current_dir().expect("应有当前目录");
    let entry = ProjectEntry {
        paths: vec![
            "/definitely/missing/zcv-project".into(),
            current.to_string_lossy().to_string(),
        ],
    };
    assert_eq!(valid_project_paths(&entry), Some(vec![current]));
}

#[test]
fn valid_project_paths_returns_none_when_all_stale() {
    let entry = ProjectEntry {
        paths: vec![
            "/definitely/missing/a".into(),
            "/definitely/missing/b".into(),
        ],
    };
    assert_eq!(valid_project_paths(&entry), None);
}

#[test]
fn legacy_single_path_entries_migrate_to_paths() {
    // 旧版最近项目文件是 {"path": "..."}；读取时迁移为单元素集合，下次写盘即为新格式。
    let legacy: ProjectEntry = serde_json::from_str(r#"{"path": "/tmp/legacy-project"}"#).unwrap();
    assert_eq!(legacy.paths, ["/tmp/legacy-project"]);

    let current: ProjectEntry = serde_json::from_str(
        &serde_json::to_string(&ProjectEntry {
            paths: vec!["/tmp/a".into(), "/tmp/b".into()],
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(current.paths, ["/tmp/a", "/tmp/b"]);
}

#[test]
fn multi_root_entries_join_root_names_in_label() {
    let entry = ProjectEntry {
        paths: vec!["/tmp/workspace-a".into(), "/tmp/workspace-b".into()],
    };
    assert_eq!(entry.label(), "workspace-a、workspace-b");
    assert_eq!(entry.search_text(), "/tmp/workspace-a /tmp/workspace-b");
}
