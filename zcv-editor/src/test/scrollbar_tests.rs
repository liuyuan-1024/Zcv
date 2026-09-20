use super::*;
use DiffHunkKind::*;
use zcv_buffer_diff::DiffHunkKind;

fn track_bounds(height: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(0.), px(0.)),
        size: size(px(15.), px(height)),
    }
}

#[test]
fn marker_geometry_maps_rows_to_track_absolutely() {
    // per_pixel=2（内容 200px ↔ 轨道 100px）：行 5（content 125）→ 轨道 125/2=62.5。
    // 绝对定位：marker 表示行在文档中的位置，不随滚动变化（与 thumb 同一坐标系）。
    let track = track_bounds(100.);
    let markers = marker_geometry(
        [(
            5..6,
            ScrollbarMarkerKind::Git(EditorHunkMarkerKind::Diff(Modified)),
        )],
        track,
        2.0,
        px(25.),
    );
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].y_range.start, px(62.5));
    assert_eq!(markers[0].y_range.end, px(62.5 + 12.5));

    // 拖动 thumb 到 marker 处：scroll_top = 62.5 × 2 = 125 = 行 5 的 content_y（精确对齐）。
    assert_eq!(px(62.5) * 2.0, px(125.));
}

#[test]
fn marker_geometry_enforces_minimum_height() {
    // 内容高度 == 视口高度（per_pixel=0）：行 0 的 5px 高 marker 直接映射。
    let track = track_bounds(200.);
    let markers = marker_geometry(
        [(
            0..1,
            ScrollbarMarkerKind::Git(EditorHunkMarkerKind::Diff(Added)),
        )],
        track,
        0.0,
        px(25.),
    );
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].y_range.start, px(0.));
    // 行 0 的 content 区间 [0, 25)，不足 5px？不——25px > 5px，无需夹取。
    assert_eq!(markers[0].y_range.end, px(25.));

    // 极端缩放（内容远大于视口）下单行被压到 < 5px → 夹取到 5px。
    let track = track_bounds(200.);
    let markers = marker_geometry(
        [(
            0..1,
            ScrollbarMarkerKind::Git(EditorHunkMarkerKind::Diff(Added)),
        )],
        track,
        40.0,
        px(25.),
    );
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].y_range.end - markers[0].y_range.start, px(5.));
}

#[test]
fn marker_geometry_merges_adjacent_same_kind_and_keeps_different_kinds() {
    let track = track_bounds(200.);
    // 同色相邻（间隙 0）合并。
    let markers = marker_geometry(
        [
            (
                0..1,
                ScrollbarMarkerKind::Git(EditorHunkMarkerKind::Diff(Added)),
            ),
            (
                1..2,
                ScrollbarMarkerKind::Git(EditorHunkMarkerKind::Diff(Added)),
            ),
        ],
        track,
        0.0,
        px(25.),
    );
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].y_range, px(0.)..px(50.));

    // 异色不合并。
    let markers = marker_geometry(
        [
            (
                0..1,
                ScrollbarMarkerKind::Git(EditorHunkMarkerKind::Diff(Added)),
            ),
            (
                1..2,
                ScrollbarMarkerKind::Git(EditorHunkMarkerKind::Diff(Deleted)),
            ),
        ],
        track,
        0.0,
        px(25.),
    );
    assert_eq!(markers.len(), 2);
}

#[test]
fn marker_geometry_discards_markers_outside_track() {
    let track = track_bounds(100.);
    // per_pixel=2：行 10（content 250..275 → track 125..137.5）超出视口 → 丢弃。
    let markers = marker_geometry(
        [(
            10..11,
            ScrollbarMarkerKind::Git(EditorHunkMarkerKind::Diff(Modified)),
        )],
        track,
        2.0,
        px(25.),
    );
    assert!(markers.is_empty());
}

#[test]
fn marker_column_x_range_divides_track_into_three_columns() {
    let track = track_bounds(200.);
    let x_range = marker_column_x_range_at(track, 0);
    // (15 − 1) / 3 = 4.67 → floor 4；从 1px 边框起。
    assert_eq!(x_range, px(1.)..px(5.));
}

#[test]
fn thumb_geometry_maps_scroll_range_to_track() {
    let track = Bounds {
        origin: point(px(10.), px(20.)),
        size: size(px(15.), px(200.)),
    };

    let (bounds, per_pixel) = thumb_geometry(track, px(300.), px(0.)).unwrap();
    // total=500，thumb=200×200/500=80，travel=120，per_pixel=300/120=2.5
    assert_eq!(per_pixel, 2.5);
    assert_eq!(bounds.size, size(px(15.), px(80.)));
    assert_eq!(bounds.origin, point(px(10.), px(20.))); // scroll_top=0 贴顶

    let (bounds, per_pixel) = thumb_geometry(track, px(300.), px(300.)).unwrap();
    assert_eq!(per_pixel, 2.5);
    assert_eq!(bounds.origin.y, px(140.)); // scroll_top=max 贴底
}

#[test]
fn thumb_geometry_is_absent_without_overflow() {
    let track = Bounds {
        origin: point(px(0.), px(0.)),
        size: size(px(15.), px(200.)),
    };
    assert_eq!(thumb_geometry(track, px(0.), px(0.)), None);
    assert_eq!(thumb_geometry(track, px(-5.), px(0.)), None);
}

#[test]
fn thumb_geometry_clamps_minimum_thumb_size() {
    let track = Bounds {
        origin: point(px(0.), px(0.)),
        size: size(px(15.), px(200.)),
    };

    let (bounds, _) = thumb_geometry(track, px(1_900.), px(0.)).unwrap();
    // total=2100，thumb=200×200/2100≈19 < 25 → 夹到 25
    assert_eq!(bounds.size.height, px(25.));

    let (_, per_pixel) = thumb_geometry(track, px(1_900.), px(1_900.)).unwrap();
    assert_eq!(per_pixel, 1_900. / 175.);
}

#[test]
fn stale_marker_result_is_discarded_after_display_version_advances() {
    let mut state = ScrollbarMarkerState::default();
    let groups = [
        Some(Arc::from(vec![ScrollbarMarker {
            y_range: px(0.)..px(5.),
            kind: ScrollbarMarkerKind::Search,
        }])),
        None,
    ];

    // 计算版本 1，安装时当前显示版本已推进到 2：过期结果丢弃并保持 dirty。
    state.finish_refresh(track_bounds(100.).size, 1, 2, groups.clone());
    assert!(state.marker_groups[0].is_none());
    assert!(state.dirty);

    // 版本一致时正常安装。
    state.finish_refresh(track_bounds(100.).size, 2, 2, groups);
    assert!(state.marker_groups[0].is_some());
}
