use gpui::{
    Modifiers, MouseButton, Pixels, ScrollDelta, ScrollWheelEvent, TestAppContext, point, px,
};
use zcv_multi_buffer::MultiBufferExcerpt;
use zcv_text::ByteOffset;

use super::common::{scrollbar_geometry, scrolling_text, test_buffer};
use super::*;
use crate::scroll::ScrollbarThumbState;
use crate::scrollbar::marker_geometry;
use crate::selection::SelectionSet;

#[gpui::test]
fn composite_refresh_restores_scroll_from_source_anchor(cx: &mut TestAppContext) {
    let first = test_buffer(
        cx,
        (0..40)
            .map(|row| format!("first {row}\n"))
            .collect::<String>(),
    );
    let second = test_buffer(
        cx,
        (0..80)
            .map(|row| format!("second {row}\n"))
            .collect::<String>(),
    );
    cx.update_entity(&first, |buffer, cx| {
        buffer.set_file_path("first.rs".into(), cx)
    });
    cx.update_entity(&second, |buffer, cx| {
        buffer.set_file_path("second.rs".into(), cx)
    });
    let first = cx.new(|cx| MultiBuffer::singleton(first, cx));
    let second = cx.new(|cx| MultiBuffer::singleton(second, cx));
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                MultiBufferExcerpt::line_range(first, 0..40, cx),
                MultiBufferExcerpt::line_range(second.clone(), 0..80, cx),
            ],
            cx,
        );
    });
    let (editor, cx) = cx.add_window_view({
        let combined = combined.clone();
        move |_, cx| Editor::for_multi_buffer(combined, cx)
    });
    cx.run_until_parked();
    cx.refresh().expect("测试窗口应可刷新");
    let line_height = cx.update(|window, _| window.line_height());
    let (scroll_anchor, old_output_offset) = cx.update_entity(&editor, |editor, cx| {
        assert!(editor.scroll_to(line_height * 70., cx));
        (
            editor
                .capture_scroll_anchor(cx)
                .expect("组合视口应能锚定到底层文件"),
            editor
                .display_map
                .display_point_to_offset(editor.scroll_anchor())
                .expect("旧视口顶部应能映射到组合偏移"),
        )
    });
    let old_location = cx.read_entity(&combined, |buffer, _| {
        buffer
            .location_for_offset(old_output_offset)
            .expect("旧视口顶部应映射到底层文件")
    });

    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(vec![MultiBufferExcerpt::line_range(second, 20..70, cx)], cx);
    });
    cx.update_entity(&editor, |editor, cx| {
        assert!(editor.restore_scroll_anchor(scroll_anchor, cx));
        assert_ne!(editor.scroll_anchor().row().get(), 0);
    });
    let new_output_offset = cx.read_entity(&editor, |editor, _| {
        editor
            .display_map
            .display_point_to_offset(editor.scroll_anchor())
            .expect("新视口顶部应能映射到组合偏移")
    });
    let new_location = cx.read_entity(&combined, |buffer, _| {
        buffer
            .location_for_offset(new_output_offset)
            .expect("新视口顶部应映射到底层文件")
    });
    assert_eq!(new_location, old_location);
}

#[gpui::test]
fn composite_refresh_keeps_the_viewport_on_a_virtual_file_header(cx: &mut TestAppContext) {
    let source = test_buffer(
        cx,
        (0..80)
            .map(|row| format!("line {row}\n"))
            .collect::<String>(),
    );
    cx.update_entity(&source, |buffer, cx| {
        buffer.set_file_path("header.rs".into(), cx)
    });
    let source = cx.new(|cx| MultiBuffer::singleton(source, cx));
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![MultiBufferExcerpt::line_range(source.clone(), 0..60, cx)],
            cx,
        );
    });
    let (editor, cx) = cx.add_window_view({
        let combined = combined.clone();
        move |_, cx| Editor::for_multi_buffer(combined, cx)
    });
    cx.run_until_parked();
    cx.refresh().expect("测试窗口应可刷新");

    let anchor = cx.update_entity(&editor, |editor, cx| {
        assert_eq!(editor.scroll_anchor().row(), DisplayRow::ZERO);
        editor
            .capture_scroll_anchor(cx)
            .expect("文件标题应能锚定到底层文件")
    });
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(vec![MultiBufferExcerpt::line_range(source, 10..70, cx)], cx);
    });
    cx.update_entity(&editor, |editor, cx| {
        assert!(editor.restore_scroll_anchor(anchor, cx));
        assert_eq!(editor.scroll_anchor().row(), DisplayRow::ZERO);
    });
}

#[gpui::test]
fn moving_caret_beyond_viewport_scrolls_it_back_into_view(cx: &mut TestAppContext) {
    let text = (0..120)
        .map(|row| format!("line {row}\n"))
        .collect::<String>();
    let buffer = test_buffer(cx, text);
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });

    cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));
    for _ in 0..80 {
        cx.dispatch_action(MoveDown);
    }
    cx.run_until_parked();

    cx.read_entity(&editor, |editor, _| {
        let caret = editor.selections().primary().head();
        let caret_row = editor
            .render_snapshot()
            .byte_to_position(caret)
            .expect("caret 应保持有效")
            .line()
            .get();
        assert_eq!(caret_row, 80);
        assert!(editor.scroll_manager.anchor().row().get() > 0);
        assert!(editor.scroll_manager.anchor().row().get() <= caret_row);
    });
}
#[gpui::test]
fn vertical_movement_preserves_goal_column_across_short_rows(cx: &mut TestAppContext) {
    let text = "a long line with enough text\nshort\nanother long line here\n";
    let buffer = test_buffer(cx, text);
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });
    cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));

    // 水平移动到列 10（水平移动清除 goal）。
    for _ in 0..10 {
        cx.dispatch_action(MoveRight);
    }
    cx.run_until_parked();

    // 垂直移动到短行：列被钳制到行尾，但 goal 保留 10。
    cx.dispatch_action(MoveDown);
    cx.run_until_parked();
    let (short_row_column, goal) = cx.read_entity(&editor, |editor, _| {
        let position = editor
            .render_snapshot()
            .byte_to_position(editor.selections().primary().head())
            .expect("caret 应有效");
        (
            position.column().get(),
            editor.selections().primary().goal(),
        )
    });
    assert_eq!(short_row_column, "short".len());
    assert_eq!(goal, Some(10));

    // 再垂直移动到长行：光标回到持久化的目标列 10。
    cx.dispatch_action(MoveDown);
    cx.run_until_parked();
    cx.read_entity(&editor, |editor, _| {
        let position = editor
            .render_snapshot()
            .byte_to_position(editor.selections().primary().head())
            .expect("caret 应有效");
        assert_eq!(position.column().get(), 10);
    });
}
#[gpui::test]
fn wheel_input_updates_editor_scroll_state(cx: &mut TestAppContext) {
    let text = (0..120)
        .map(|row| format!("line {row}\n"))
        .collect::<String>();
    let buffer = test_buffer(cx, text);
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });

    cx.run_until_parked();
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(4.), px(4.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-120.))),
        ..Default::default()
    });

    cx.read_entity(&editor, |editor, _| {
        assert!(
            editor.scroll_manager.anchor().row() > DisplayRow::ZERO
                || editor.scroll_manager.offset().y > px(0.)
        );
    });
}
#[gpui::test]
fn horizontal_scroll_stops_at_content_edge_and_caret_autoscrolls(cx: &mut TestAppContext) {
    let text = "修改 zcv 模块时，请先阅读 zcv/docs/下的所有文档规范。同时查阅**[zed编辑器](https://github.com/zed-industries/zed)**的源码，看看zed是如何实现的，参考zed的实现方式，甚至是直接照搬zed的实现方式。".repeat(4);
    let buffer = test_buffer(cx, text.clone());
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });

    cx.run_until_parked();
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(4.), px(4.)),
        delta: ScrollDelta::Pixels(point(px(-100_000.), px(0.))),
        ..Default::default()
    });
    let maximum = cx.read_entity(&editor, |editor, _| editor.scroll_manager.offset().x);
    assert!(maximum > px(0.));

    cx.simulate_event(ScrollWheelEvent {
        position: point(px(4.), px(4.)),
        delta: ScrollDelta::Pixels(point(px(-100_000.), px(0.))),
        ..Default::default()
    });
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.scroll_manager.offset().x, maximum);
    });

    cx.update_entity(&editor, |editor, cx| {
        editor.scroll_manager.scroll_by(point(px(100_000.), px(0.)));
        editor.set_selections(SelectionSet::caret(ByteOffset::new(text.len())));
        editor.request_autoscroll();
        cx.notify();
    });
    cx.run_until_parked();
    cx.read_entity(&editor, |editor, _| {
        let scroll_left = editor.scroll_manager.offset().x;
        assert!(scroll_left > px(0.));
        assert!(scroll_left <= maximum);
        let cursor = editor
            .pixel_position_of_newest_cursor
            .expect("行尾光标应有布局位置");
        let bounds = editor.last_bounds.expect("Editor 应保存最近布局范围");
        assert!(cursor.x + px(2.) <= bounds.size.width);
    });
}
#[gpui::test]
fn clicking_scrollbar_track_pages_and_enters_dragging(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, scrolling_text());
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });
    cx.run_until_parked();

    let (track_bounds, _, _) = scrollbar_geometry(&editor, cx);
    assert!(
        cx.read_entity(&editor, |editor, _| editor.max_scroll_top()) > Pixels::ZERO,
        "100 行应超过视口高度"
    );
    let click_y = track_bounds.origin.y + track_bounds.size.height * 0.75;

    // 点击 thumb 下方轨道：应以点击处为中心跳页，并进入拖动态。
    cx.simulate_mouse_down(
        point(track_bounds.origin.x + px(7.5), click_y),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.scrollbar_thumb_state(),
            ScrollbarThumbState::Dragging,
            "点击轨道应进入拖动态"
        );
        let scroll_top = editor.scroll_top();
        assert!(scroll_top > Pixels::ZERO, "点击轨道应产生滚动");
        assert!(scroll_top <= editor.max_scroll_top());
        assert_eq!(
            editor.selections().primary().head(),
            ByteOffset::ZERO,
            "点击滚动轴不应移动光标"
        );
    });

    // 重绘后注册 MouseUp handler，在轨道内松开应回到 Hovered。
    cx.refresh().expect("测试窗口应可刷新");
    cx.simulate_mouse_up(
        point(track_bounds.origin.x + px(7.5), click_y),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.scrollbar_thumb_state(),
            ScrollbarThumbState::Hovered,
            "在轨道内松开应回到 Hovered"
        );
    });
}
#[gpui::test]
fn dragging_scrollbar_thumb_moves_content_by_delta(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, scrolling_text());
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });
    cx.run_until_parked();

    let (_, thumb_bounds, per_pixel) = scrollbar_geometry(&editor, cx);
    let thumb_bounds = thumb_bounds.expect("内容超视口时应有 thumb");
    let thumb_center = point(
        thumb_bounds.origin.x + thumb_bounds.size.width * 0.5,
        thumb_bounds.origin.y + thumb_bounds.size.height * 0.5,
    );

    // 悬停 → Hovered。
    cx.simulate_mouse_move(thumb_center, None, Modifiers::default());
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.scrollbar_thumb_state(), ScrollbarThumbState::Hovered);
    });

    // 按下 thumb 中心 → 重绘注册 MouseUp → 向下拖动 50px。
    cx.simulate_mouse_down(thumb_center, MouseButton::Left, Modifiers::default());
    cx.refresh().expect("测试窗口应可刷新");
    let scroll_before = cx.read_entity(&editor, |editor, _| editor.scroll_top());
    cx.simulate_mouse_move(
        point(thumb_center.x, thumb_center.y + px(50.)),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    cx.read_entity(&editor, |editor, _| {
        let expected = scroll_before + px(50.) * per_pixel;
        let delta = (editor.scroll_top() - expected).abs() / px(1.);
        assert!(
            delta < 1.0,
            "拖动 50px 应滚动约 {}px，实际差 {delta}px",
            px(50.) * per_pixel,
        );
        assert_eq!(
            editor.scrollbar_thumb_state(),
            ScrollbarThumbState::Dragging
        );
    });

    // 松开结束拖动。
    cx.refresh().expect("测试窗口应可刷新");
    cx.simulate_mouse_up(
        point(thumb_center.x, thumb_center.y + px(50.)),
        MouseButton::Left,
        Modifiers::default(),
    );
}
#[gpui::test]
fn dragging_thumb_to_marker_position_scrolls_to_that_row(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, scrolling_text());
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });
    cx.run_until_parked();

    // 注入行 50 的 diff hunk（行 50 内容 y = 50 × line_height）。
    editor.update(cx, |editor, cx| {
        editor.set_diff_hunks(
            vec![DiffHunk {
                range: 50..51,
                old_range: 50..51,
                kind: DiffHunkKind::Modified,
            }],
            cx,
        );
    });
    cx.run_until_parked();

    let (track_bounds, thumb_bounds, per_pixel) = scrollbar_geometry(&editor, cx);
    let thumb_bounds = thumb_bounds.expect("内容超视口时应有 thumb");
    // marker 的轨道位置（绝对定位：行 50 在文档中的位置）。
    let markers = marker_geometry(
        [(50..51, DiffHunkKind::Modified)],
        track_bounds,
        per_pixel,
        cx.update(|window, _| window.line_height()),
    );
    let marker_y = markers[0].y_range.start;
    let track_x = track_bounds.origin.x + px(7.5);

    // 从 thumb 顶（scroll_top=0）拖到 marker 位置：scroll_top 应精确等于 marker 行的内容 y。
    cx.simulate_mouse_down(
        point(track_x, thumb_bounds.origin.y),
        MouseButton::Left,
        Modifiers::default(),
    );
    cx.refresh().expect("测试窗口应可刷新");
    cx.simulate_mouse_move(
        point(track_x, marker_y),
        Some(MouseButton::Left),
        Modifiers::default(),
    );
    cx.read_entity(&editor, |editor, _| {
        let expected = marker_y * per_pixel;
        let delta = (editor.scroll_top() - expected).abs() / px(1.);
        assert!(
            delta < 1.0,
            "thumb 拖到 marker 处应精确滚动到该行（{}px），实际差 {delta}px",
            expected,
        );
    });
}
#[gpui::test]
fn hovering_thumb_cycles_three_states(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, scrolling_text());
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_language_buffer(buffer, cx)
    });
    cx.run_until_parked();

    let (_, thumb_bounds, _) = scrollbar_geometry(&editor, cx);
    let thumb_bounds = thumb_bounds.expect("内容超视口时应有 thumb");
    let thumb_center = point(
        thumb_bounds.origin.x + thumb_bounds.size.width * 0.5,
        thumb_bounds.origin.y + thumb_bounds.size.height * 0.5,
    );

    // 移到 thumb 上 → Hovered。
    cx.simulate_mouse_move(thumb_center, None, Modifiers::default());
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.scrollbar_thumb_state(), ScrollbarThumbState::Hovered);
    });

    // 移到文本区 → 兜底复位为 Idle。
    cx.simulate_mouse_move(point(px(100.), px(100.)), None, Modifiers::default());
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.scrollbar_thumb_state(), ScrollbarThumbState::Idle);
    });

    // 按下 → Dragging；重绘后松开（仍在 thumb 上）→ Hovered。
    cx.simulate_mouse_down(thumb_center, MouseButton::Left, Modifiers::default());
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.scrollbar_thumb_state(),
            ScrollbarThumbState::Dragging
        );
    });
    cx.refresh().expect("测试窗口应可刷新");
    cx.simulate_mouse_up(thumb_center, MouseButton::Left, Modifiers::default());
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.scrollbar_thumb_state(), ScrollbarThumbState::Hovered);
    });
}
