use super::*;
use crate::layout_state;

fn windowed(x: f32, y: f32, width: f32, height: f32) -> WindowBounds {
    WindowBounds::Windowed(Bounds {
        origin: point(px(x), px(y)),
        size: size(px(width), px(height)),
    })
}

#[test]
fn round_trip_keeps_all_variants() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bounds.json");
    for (bounds, root) in [
        (windowed(100.0, 200.0, 800.0, 600.0), Some("项目A")),
        (
            WindowBounds::Maximized(Bounds {
                origin: point(px(30.0), px(40.0)),
                size: size(px(1200.0), px(900.0)),
            }),
            Some("项目B"),
        ),
        (
            WindowBounds::Fullscreen(Bounds {
                origin: point(px(0.0), px(0.0)),
                size: size(px(2560.0), px(1440.0)),
            }),
            None,
        ),
    ] {
        save_to(&path, root.map(Path::new), bounds, Some("显示器-1".into())).unwrap();
        assert_eq!(
            load_from(&path, root.map(Path::new)),
            Some((bounds, Some("显示器-1".into())))
        );
    }
}

#[test]
fn project_record_takes_priority_over_default() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bounds.json");
    let project_a = Path::new("/项目/A");
    let project_b = Path::new("/项目/B");

    save_to(
        &path,
        Some(project_a),
        windowed(1.0, 2.0, 300.0, 400.0),
        None,
    )
    .unwrap();
    save_to(
        &path,
        Some(project_b),
        windowed(5.0, 6.0, 700.0, 800.0),
        None,
    )
    .unwrap();

    // 项目 A 读回自己的记录，而不是最近一次写入的全局默认。
    assert_eq!(
        load_from(&path, Some(project_a)),
        Some((windowed(1.0, 2.0, 300.0, 400.0), None))
    );
    // 没有记录的项目回退到全局默认（最近一次保存）。
    assert_eq!(
        load_from(&path, None),
        Some((windowed(5.0, 6.0, 700.0, 800.0), None))
    );
}

#[test]
fn missing_corrupted_or_foreign_version_returns_none() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.json");
    assert_eq!(load_from(&missing, None), None);

    let corrupted = directory.path().join("corrupted.json");
    std::fs::write(&corrupted, "{不是 JSON").unwrap();
    assert_eq!(load_from(&corrupted, None), None);

    let foreign = directory.path().join("foreign.json");
    save_to(&foreign, None, windowed(1.0, 2.0, 300.0, 400.0), None).unwrap();
    std::fs::write(&foreign, r#"{"version":999,"default":null,"projects":{}}"#).unwrap();
    assert_eq!(load_from(&foreign, None), None);
}

#[test]
fn saving_writes_default_and_project_record_with_shared_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bounds.json");
    let root = Path::new("/项目/身份");

    save_to(&path, Some(root), windowed(1.0, 2.0, 300.0, 400.0), None).unwrap();

    let file: WindowBoundsFile =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(file.version, WINDOW_BOUNDS_VERSION);
    assert!(file.default.is_some());
    assert_eq!(
        file.default.as_ref().unwrap().coordinate_space,
        WindowCoordinateSpace::GpuiLogicalPixels
    );
    // 项目键与布局文件的文件名哈希一致，保证两个文件域身份统一。
    assert_eq!(
        file.projects.keys().next().map(String::as_str),
        Some(persistence::workspace_identity(Some(root)).as_str())
    );
    assert_eq!(
        file.projects.keys().next().map(String::as_str),
        layout_state::path_for_workspace(Some(root))
            .file_stem()
            .and_then(|name| name.to_str())
    );
}

#[test]
fn display_uuid_survives_round_trip() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bounds.json");
    save_to(
        &path,
        None,
        windowed(1.0, 2.0, 300.0, 400.0),
        Some("uuid-123".into()),
    )
    .unwrap();
    assert_eq!(
        load_from(&path, None),
        Some((windowed(1.0, 2.0, 300.0, 400.0), Some("uuid-123".into())))
    );
}

#[test]
fn restore_bounds_are_clamped_without_changing_window_state() {
    let display = Bounds {
        origin: point(px(-1920.0), px(0.0)),
        size: size(px(1920.0), px(1080.0)),
    };
    let bounds = WindowBounds::Maximized(Bounds {
        origin: point(px(-4000.0), px(-2000.0)),
        size: size(px(2400.0), px(1200.0)),
    });

    assert_eq!(
        clamp_window_bounds(normalize_window_bounds(bounds), display),
        WindowBounds::Maximized(Bounds {
            origin: point(px(-1920.0), px(0.0)),
            size: size(px(1920.0), px(1080.0)),
        })
    );
}

#[test]
fn invalid_persisted_size_is_normalized_before_display_lookup() {
    let bounds = normalize_window_bounds(WindowBounds::Fullscreen(Bounds {
        origin: point(px(-20.0), px(-30.0)),
        size: size(px(-1.0), px(0.0)),
    }));

    assert_eq!(
        bounds,
        WindowBounds::Fullscreen(Bounds {
            origin: point(px(-20.0), px(-30.0)),
            size: size(px(1.0), px(1.0)),
        })
    );
}
