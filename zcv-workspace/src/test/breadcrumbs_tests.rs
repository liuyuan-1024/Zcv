use std::path::Path;

use gpui::SharedString;
use zcv_path::{AbsolutePathBuf, DisplayPath, ProjectRootDisplay, ProjectRootId, RelativePathBuf};

use super::breadcrumb_segments;

#[test]
fn multi_root_breadcrumbs_prefix_the_disambiguated_root_label() {
    let directory = tempfile::tempdir().expect("应创建临时项目根目录");
    let full_path = AbsolutePathBuf::canonicalize(directory.path()).expect("项目根应可规范化");
    let display_path = DisplayPath::new(
        ProjectRootDisplay {
            id: ProjectRootId::new(1),
            label: "apps/frontend".into(),
            full_path,
        },
        RelativePathBuf::from_path(Path::new("src/main.rs")).expect("相对路径应有效"),
    );
    let segments = vec![SharedString::from("src"), SharedString::from("main.rs")];

    let with_root = breadcrumb_segments(segments.clone(), Some(&display_path), true)
        .into_iter()
        .map(|segment| segment.to_string())
        .collect::<Vec<_>>();
    assert_eq!(with_root, ["apps/frontend", "src", "main.rs"]);

    let without_root = breadcrumb_segments(segments, Some(&display_path), false)
        .into_iter()
        .map(|segment| segment.to_string())
        .collect::<Vec<_>>();
    assert_eq!(without_root, ["src", "main.rs"]);
}
