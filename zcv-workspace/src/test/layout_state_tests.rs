use gpui::px;

use super::*;
use crate::dock::DockData;

#[test]
fn layout_round_trip_and_version_guard() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("layout.json");
    let layout = WorkspaceLayout {
        version: LAYOUT_VERSION,
        docks: DockStructure {
            left: DockData {
                visible: true,
                active_panel: Some("project-tree".into()),
                size: Some(f32::from(px(320.0))),
            },
            ..DockStructure::default()
        },
        pane: SerializedPane {
            items: vec![
                SerializedPaneItem::Source(PathBuf::from("a.txt")),
                SerializedPaneItem::Preview(PathBuf::from("b.txt")),
                SerializedPaneItem::StandalonePreview(PathBuf::from("image.png")),
                SerializedPaneItem::Custom {
                    kind: "project-diff".into(),
                    state: serde_json::json!({ "kind": "staged" }),
                },
            ],
            active_item: Some(2),
        },
        panels: Vec::new(),
    };
    save(&path, &layout).unwrap();
    assert_eq!(load(&path), Some(layout));

    // 版本不匹配（旧版或未知版本）一律不加载，回到全新默认布局。
    fs::write(
        &path,
        r#"{"version":1,"docks":{"left":{"visible":true,"active_panel":"project-tree","size":320.0},"right":{"visible":false},"bottom":{"visible":false}},"pane":{"items":[],"active_item":null},"panels":[]}"#,
    )
    .unwrap();
    assert_eq!(load(&path), None);

    fs::write(&path, r#"{"version":999,"docks":{}}"#).unwrap();
    assert_eq!(load(&path), None);
}
