use std::path::PathBuf;

use gpui::SharedString;
use zcv_path::{AbsolutePathBuf, DisplayPath, ProjectRootDisplay, ProjectRootId, RelativePathBuf};

use super::breadcrumb_segments;

#[test]
fn multi_root_breadcrumbs_prefix_the_disambiguated_root_label() {
    let display_path = DisplayPath::new(
        ProjectRootDisplay {
            id: ProjectRootId::new(1),
            label: "apps/frontend".into(),
            full_path: AbsolutePathBuf::new(PathBuf::from("/work/apps/frontend"))
                .expect("绝对路径应有效"),
        },
        RelativePathBuf::from_path(std::path::Path::new("src/main.rs")).expect("相对路径应有效"),
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
