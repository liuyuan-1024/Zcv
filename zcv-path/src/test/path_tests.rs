use super::*;

#[test]
fn tree_order_groups_directories_and_sorts_names_naturally() {
    let mut entries = [
        (Path::new("src/file10.rs"), false),
        (Path::new("src/a.rs"), false),
        (Path::new("src/Zip"), true),
        (Path::new("src/file2.rs"), false),
        (Path::new("src/apple"), true),
        (Path::new("src/File2.rs"), false),
    ];
    entries.sort_by(|&left, &right| compare_tree_entries(left, right));
    assert_eq!(
        entries.map(|(path, _)| path),
        [
            Path::new("src/apple"),
            Path::new("src/Zip"),
            Path::new("src/a.rs"),
            Path::new("src/file2.rs"),
            Path::new("src/File2.rs"),
            Path::new("src/file10.rs"),
        ]
    );

    let mut files = [
        Path::new("a.rs"),
        Path::new("src/z.rs"),
        Path::new("src/B.rs"),
    ];
    files.sort_by(|&left, &right| compare_tree_entries((left, false), (right, false)));
    assert_eq!(
        files,
        [
            Path::new("src/B.rs"),
            Path::new("src/z.rs"),
            Path::new("a.rs")
        ]
    );
}

#[test]
fn natural_path_order_does_not_group_directories_before_files() {
    let mut paths = [
        Path::new("src/sub/b.rs"),
        Path::new("a.rs"),
        Path::new("src/file10.rs"),
        Path::new("src/file2.rs"),
    ];
    paths.sort_by(|&left, &right| compare_natural_paths(left, right));
    assert_eq!(
        paths,
        [
            Path::new("a.rs"),
            Path::new("src/file2.rs"),
            Path::new("src/file10.rs"),
            Path::new("src/sub/b.rs"),
        ]
    );
}

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
    assert!(RelativePathBuf::from_path(Path::new("/tmp/file")).is_err());
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
        Some(RelativePathBuf::from_path(Path::new("src/main.rs")).unwrap())
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
