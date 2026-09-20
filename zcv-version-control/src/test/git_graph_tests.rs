use std::sync::Arc;

use zcv_language::LanguageRegistry;

use super::*;

/// 行高必须跟随内容字号通道：内容字号放大后行高应变大。
/// 若走错通道，`cmd-=`（workspace::IncreaseContentFontSize）对版本控制图没有任何可见效果。
#[gpui::test]
fn row_height_follows_content_font_size(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let original = f32::from(typography::content_size(cx));
        let baseline = row_height(typography::content_line(cx));

        typography::set_base_typography(cx, Some(original + 4.), None, None);
        let enlarged = row_height(typography::content_line(cx));
        // 临时调整基础字号，验证行高随字号变化；测试结束后立即还原。
        typography::set_base_typography(cx, Some(original), None, None);

        assert!(
            enlarged > baseline,
            "内容字号 {original} → {} 应放大行高，实际 {baseline:?} → {enlarged:?}",
            original + 4.
        );
        assert_eq!(
            row_height(typography::content_line(cx)),
            baseline,
            "还原内容字号后行高应回到原值（字号以 f32 存储，往返无损）"
        );
    });
}

#[test]
fn column_resize_preserves_total_width_and_minimums() {
    let mut widths = [px(140.0), px(400.0), px(100.0), px(120.0), px(100.0)];
    let total_before: f32 = widths.iter().map(|width| f32::from(*width)).sum();

    resize_column_widths(&mut widths, 0, px(-500.0));

    let total_after: f32 = widths.iter().map(|width| f32::from(*width)).sum();
    assert!((total_before - total_after).abs() < f32::EPSILON);
    assert_eq!(widths[0], COLUMN_MIN_WIDTH);
    assert_eq!(widths[1], px(524.0));
}

#[test]
fn graph_column_width_uses_only_actual_lane_count() {
    let row = |max_lanes| GraphRow {
        commit: GraphCommit {
            oid: String::new(),
            parents: Vec::new(),
            author_name: String::new(),
            timestamp: 0,
            subject: String::new(),
            refs: Vec::new(),
        },
        layout: GraphRowLayout {
            dot_lane: 0,
            dot_color: 0,
            lines: Vec::new(),
            max_lanes,
        },
    };

    assert_eq!(graph_column_width(&[row(1)]), px(16.0));
    assert_eq!(graph_column_width(&[row(1), row(3)]), px(48.0));
    assert_eq!(graph_column_width(&[]), px(0.0));
}

#[test]
fn column_resize_respects_maximums_on_both_sides() {
    let mut widths = [px(100.0), px(500.0), px(100.0), px(120.0), px(100.0)];
    resize_column_widths(&mut widths, 0, px(500.0));
    assert_eq!(widths[0], COLUMN_RESIZE_MAX_WIDTHS[0]);
    assert_eq!(widths[1], px(280.0));

    let mut widths = [px(200.0), px(700.0), px(100.0), px(120.0), px(100.0)];
    resize_column_widths(&mut widths, 0, px(-500.0));
    assert_eq!(widths[0], px(100.0));
    assert_eq!(widths[1], COLUMN_RESIZE_MAX_WIDTHS[1]);
}

#[test]
fn column_resize_ignores_an_invalid_existing_pair() {
    let mut widths = [px(8.0), px(8.0), px(100.0), px(120.0), px(100.0)];
    assert_eq!(resize_column_widths(&mut widths, 0, px(10.0)), px(0.0));
    assert_eq!(widths[0], px(8.0));
    assert_eq!(widths[1], px(8.0));
}

#[test]
fn double_click_reset_restores_left_column_and_preserves_total_width() {
    let mut widths = [px(220.0), px(420.0), px(120.0), px(112.0), px(96.0)];
    let total_before: f32 = widths.iter().map(|width| f32::from(*width)).sum();

    reset_column_width(&mut widths, 0, px(160.0));

    let total_after: f32 = widths.iter().map(|width| f32::from(*width)).sum();
    assert_eq!(widths[0], px(160.0));
    assert_eq!(widths[1], px(480.0));
    assert!((total_before - total_after).abs() < f32::EPSILON);
}

/// 回归：提交图视图与其搜索栏之间不得互相强引用。
///
/// C4 后视图持有 SearchBar，SearchBar 又强持有视图作为搜索目标，构成环；
/// 关闭标签/面板（不触发活动 Item 变化、因而不会清 target）时两者都无法释放。
/// 目标改为弱句柄后，释放外部强引用即可让视图与搜索栏一起释放。
#[gpui::test]
fn git_graph_view_and_search_bar_release_together(cx: &mut gpui::TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let project = cx.new(|cx| {
        Project::new(
            directory.path().to_path_buf(),
            Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let view = cx.new(|cx| GitGraphView::new(project, cx));
    let search_bar = cx.read_entity(&view, |view, _| view.search_bar.clone());
    let weak_view = view.downgrade();
    let weak_search_bar = search_bar.downgrade();

    // 模拟工具项激活：搜索栏把视图登记为自搜索目标。
    let (_, visual) = cx.add_window_view(|window, cx| {
        let toolbar = cx.new(|_| GitGraphToolbar::new());
        toolbar.update(cx, |toolbar, cx| {
            toolbar.set_active_pane_item(Some(&view as &dyn ItemHandle), window, cx);
        });
        gpui::Empty
    });
    visual.run_until_parked();

    drop(search_bar);
    drop(view);
    // 实体释放分多轮 effect 完成：逐轮刷新直到视图与搜索栏都被回收。
    for _ in 0..4 {
        visual.update(|_, _| {});
        visual.run_until_parked();
    }

    assert!(
        weak_view.upgrade().is_none(),
        "提交图视图在外部强引用释放后应被回收（不再被搜索栏强持有）"
    );
    assert!(
        weak_search_bar.upgrade().is_none(),
        "搜索栏应随视图一起释放"
    );
}
