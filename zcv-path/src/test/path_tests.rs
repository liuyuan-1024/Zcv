use super::*;

#[test]
fn relative_paths_use_unix_separators() {
    assert_eq!(
        RelativePathBuf::from_path_with_style(
            Path::new(r"src\\./editor/../main.rs"),
            PathStyle::Windows,
        )
        .unwrap()
        .as_str(),
        "src/main.rs"
    );
}

#[test]
fn relative_paths_reject_absolute_paths() {
    assert!(RelativePathBuf::from_unix_str("/tmp/file").is_err());
    assert!(
        RelativePathBuf::from_path_with_style(Path::new(r"C:\\tmp\\file"), PathStyle::Windows,)
            .is_err()
    );
}

#[test]
fn canonicalize_returns_an_absolute_path_without_verbatim_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let path = AbsolutePathBuf::canonicalize(directory.path()).unwrap();
    assert!(path.as_path().is_absolute());
    assert!(!path.to_string().starts_with(r"\\?\"));
}

#[test]
fn absolute_path_relativizes_against_a_known_root() {
    let directory = tempfile::tempdir().unwrap();
    let root = AbsolutePathBuf::canonicalize(directory.path()).unwrap();
    let file = root.as_path().join("src/main.rs");
    assert_eq!(
        root.relative_path(&file),
        Some(RelativePathBuf::from_unix_str("src/main.rs").unwrap())
    );
    let outside = root.as_path().parent().unwrap().join("other/file.rs");
    assert_eq!(root.relative_path(&outside), None);
}

#[test]
fn stable_identity_preserves_unix_path_semantics() {
    assert_eq!(stable_identity(Path::new("/tmp/project")), "/tmp/project");
}

#[test]
fn normalize_for_comparison_keeps_missing_leaf_under_canonical_parent() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("missing").join("file.rs");
    let normalized = normalize_for_comparison(&path).unwrap();
    assert_eq!(
        normalized.file_name().and_then(|name| name.to_str()),
        Some("file.rs")
    );
    assert!(normalized.starts_with(dunce::canonicalize(directory.path()).unwrap()));
}
