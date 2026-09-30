use zcv_multi_buffer::MultiBufferOffset;

use gpui::{
    Context, Entity, IntoElement, Modifiers, MouseButton, Pixels, Render, ScrollDelta,
    ScrollWheelEvent, TestAppContext, Window, point, px,
};

/// 测试包装：把预先创建的 Editor 实体作为窗口视图（布局由窗口首帧触发）。
struct EditorInWindow(Entity<Editor>);

impl Render for EditorInWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.0.clone()
    }
}
use zcv_buffer_diff::{DiffHunkKind, DiffHunkStaging};
use zcv_multi_buffer::{ExcerptRange, MultiBuffer};
use zcv_text::{Line, LogicalColumn, Position};

use super::common::focus_editor;

use super::common::{inject_editor_diff, scrollbar_geometry, scrolling_text, test_buffer};
use super::*;
use crate::display_map::EditorHunkMarkerKind;
use crate::scroll::ScrollbarThumbState;
use crate::scrollbar::{ScrollbarMarkerKind, marker_geometry};
use crate::selection::SelectionSet;

#[gpui::test]
fn composite_scrollbar_refresh_cancels_tasks_and_releases_retained_inputs(cx: &mut TestAppContext) {
    let source = test_buffer(cx, scrolling_text());
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(source, 0..40, cx)], cx);
    });
    let editor = cx.new(|cx| Editor::for_multi_buffer(combined, cx));
    let track = Bounds::new(point(px(0.), px(0.)), gpui::size(px(15.), px(200.)));
    let retained = Arc::new(());
    let weak = Arc::downgrade(&retained);
    cx.update_entity(&editor, |editor, cx| {
        let snapshot = editor.display_snapshot(cx);
        let pending = cx.spawn(async move |_, _| {
            futures_lite::future::pending::<()>().await;
            drop(retained);
        });
        editor.scrollbar_marker_state.begin_refresh(pending);
        for _ in 0..3 {
            editor.refresh_scrollbar_markers(snapshot.clone(), track, 1., px(20.), cx);
            assert!(
                editor.scrollbar_marker_state.pending_refresh.is_none(),
                "组合文档应取消并停止排队滚动条标记任务"
            );
            assert!(editor.scrollbar_marker_groups().iter().all(Option::is_none));
        }
    });
    cx.run_until_parked();
    assert!(weak.upgrade().is_none(), "被取消的任务必须释放其持有的输入");
}

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
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(first, 0..40, cx)], cx);
        buffer.set_excerpts_for_path(
            vec![ExcerptRange::line_range(second.clone(), 0..80, cx)],
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
    let old_output_offset = cx.update_entity(&editor, |editor, cx| {
        assert!(editor.scroll_to(line_height * 70., cx));
        editor
            .display_snapshot(cx)
            .display_point_to_offset(editor.scroll_anchor())
            .expect("旧视口顶部应能映射到组合偏移")
    });
    let old_location = cx.read_entity(&combined, |buffer, _| {
        buffer
            .location_for_offset(old_output_offset)
            .expect("旧视口顶部应映射到底层文件")
    });

    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(second, 20..70, cx)], cx);
    });
    cx.run_until_parked();
    cx.refresh().expect("结构刷新后测试窗口应可刷新");
    // 长期滚动锚点由 ScrollManager 持有，excerpt 重建后按当前快照解析即回到原文件内容位置。
    assert_ne!(
        cx.read_entity(&editor, |editor, _| editor.scroll_anchor().row().get()),
        0
    );
    let new_output_offset = cx.read_entity(&editor, |editor, cx| {
        editor
            .display_snapshot(cx)
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
fn composite_refresh_keeps_the_viewport_anchored_to_the_file(cx: &mut TestAppContext) {
    let source = test_buffer(
        cx,
        (0..80)
            .map(|row| format!("line {row}\n"))
            .collect::<String>(),
    );
    cx.update_entity(&source, |buffer, cx| {
        buffer.set_file_path("header.rs".into(), cx)
    });
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts_for_path(
            vec![ExcerptRange::line_range(source.clone(), 0..60, cx)],
            cx,
        );
    });
    let (editor, cx) = cx.add_window_view({
        let combined = combined.clone();
        move |_, cx| Editor::for_multi_buffer(combined, cx)
    });
    cx.run_until_parked();
    cx.refresh().expect("测试窗口应可刷新");

    // 文件标题块占两个显示行，视口锚点解析到该文件首个文本行。
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.scroll_anchor().row(), DisplayRow::new(2));
    });
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(source, 10..70, cx)], cx);
    });
    cx.run_until_parked();
    cx.refresh().expect("结构刷新后测试窗口应可刷新");
    // 锚点保存的是底层文件位置；DisplayMap 把文件标题解析为 sticky 块，
    // 视口落在该文件的第一个文本行（标题占 2 行）。
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.scroll_anchor().row(), DisplayRow::new(2));
    });
}

#[gpui::test]
fn folding_a_later_file_preserves_the_viewport_anchor(cx: &mut TestAppContext) {
    let first = test_buffer(
        cx,
        (0..120)
            .map(|row| format!("first {row}\n"))
            .collect::<String>(),
    );
    let second = test_buffer(
        cx,
        (0..120)
            .map(|row| format!("second {row}\n"))
            .collect::<String>(),
    );
    cx.update_entity(&first, |buffer, cx| {
        buffer.set_file_path("first.rs".into(), cx)
    });
    cx.update_entity(&second, |buffer, cx| {
        buffer.set_file_path("second.rs".into(), cx)
    });

    let second_buffer_id = cx.read_entity(&second, |buffer, _| buffer.buffer_id());
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(first, 0..120, cx)], cx);
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(second, 0..120, cx)], cx);
    });
    let (editor, cx) = cx.add_window_view({
        let combined = combined.clone();
        move |_, cx| Editor::for_multi_buffer(combined, cx)
    });
    cx.run_until_parked();
    cx.refresh().expect("测试窗口应可刷新");

    let line_height = cx.update(|window, _| window.line_height());
    let old_output_offset = cx.update_entity(&editor, |editor, cx| {
        assert!(editor.scroll_to(line_height * 60., cx));
        editor
            .display_snapshot(cx)
            .display_point_to_offset(editor.scroll_anchor())
            .expect("折叠前视口顶部应能映射到组合偏移")
    });
    let old_location = cx.read_entity(&combined, |buffer, _| {
        buffer
            .location_for_offset(old_output_offset)
            .expect("折叠前视口顶部应映射到底层文件")
    });

    cx.update_entity(&editor, |editor, cx| {
        editor.toggle_buffer_fold(second_buffer_id, cx);
    });
    cx.run_until_parked();
    cx.refresh().expect("折叠后测试窗口应可刷新");

    let new_output_offset = cx.read_entity(&editor, |editor, cx| {
        editor
            .display_snapshot(cx)
            .display_point_to_offset(editor.scroll_anchor())
            .expect("折叠后视口顶部应能映射到组合偏移")
    });
    let new_location = cx.read_entity(&combined, |buffer, _| {
        buffer
            .location_for_offset(new_output_offset)
            .expect("折叠后视口顶部应映射到底层文件")
    });

    assert_eq!(new_location, old_location);
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

    focus_editor(&editor, cx);
    for _ in 0..80 {
        cx.dispatch_action(MoveDown);
    }
    cx.run_until_parked();

    cx.read_entity(&editor, |editor, cx| {
        let caret = editor.selections(cx).primary().head();
        let caret_row = editor
            .render_snapshot(cx)
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
    focus_editor(&editor, cx);

    // 水平移动到列 10（水平移动清除 goal）。
    for _ in 0..10 {
        cx.dispatch_action(MoveRight);
    }
    cx.run_until_parked();

    // 垂直移动到短行：列被钳制到行尾，但 goal 保留 10。
    cx.dispatch_action(MoveDown);
    cx.run_until_parked();
    let (short_row_column, goal) = cx.read_entity(&editor, |editor, cx| {
        let position = editor
            .render_snapshot(cx)
            .byte_to_position(editor.selections(cx).primary().head())
            .expect("caret 应有效");
        (
            position.column().get(),
            editor.selections(cx).primary().goal(),
        )
    });
    assert_eq!(short_row_column, "short".len());
    assert_eq!(goal, Some(10));

    // 再垂直移动到长行：光标回到持久化的目标列 10。
    cx.dispatch_action(MoveDown);
    cx.run_until_parked();
    cx.read_entity(&editor, |editor, cx| {
        let position = editor
            .render_snapshot(cx)
            .byte_to_position(editor.selections(cx).primary().head())
            .expect("caret 应有效");
        assert_eq!(position.column().get(), 10);
    });
}

/// 回归：打开文件后的导航（open_path_at / open_path_at_line_column）若发生在软换行布局之前，滚动目标点必须按布局后的显示行换算，否则长行文件会滚不到目标行。
#[gpui::test]
fn navigation_before_wrap_layout_lands_on_target_row(cx: &mut TestAppContext) {
    // 深文件 + 长行：软换行生效后目标行的显示行号远大于 buffer 行号。
    let text = (0..120)
        .map(|row| format!("line {row} {}", "x".repeat(250)))
        .collect::<Vec<_>>()
        .join("\n");
    let buffer = test_buffer(cx, text);
    // 模拟 open_path_now 新建的 Editor：尚未放入窗口、从未布局（wrap 宽度未设置）。
    let editor = cx.new(|cx| Editor::for_language_buffer(buffer, cx));
    // 真实环境默认软换行（EditorWidth）。
    editor.update(cx, |editor, cx| {
        editor.set_soft_wrap_mode(Some(SoftWrap::EditorWidth), cx);
    });
    let target_line = 100;
    let target_offset = cx.read_entity(&editor, |editor, cx| {
        editor
            .render_snapshot(cx)
            .position_to_byte(Position::new(Line::new(target_line), LogicalColumn::ZERO))
            .expect("目标行应有效")
    });
    let before_nav = cx.read_entity(&editor, |editor, cx| {
        editor.display_snapshot(cx).line_count()
    });
    assert_eq!(before_nav, 120, "导航前 Editor 未布局，不应换行");
    editor.update(cx, |editor, cx| {
        editor.select_byte_range(target_offset.get()..target_offset.get(), cx);
        editor.request_scroll_to_top(4, cx);
    });

    // 再把 Editor 放入窗口：布局（含软换行重排）发生在导航请求之后。
    let editor_in_window = editor.clone();
    cx.add_window_view(move |_, _cx| EditorInWindow(editor_in_window));
    cx.run_until_parked();
    cx.refresh().expect("测试窗口应可刷新");

    cx.read_entity(&editor, |editor, cx| {
        let head = editor.selections(cx).primary().head();
        let point = editor
            .display_snapshot(cx)
            .offset_to_display_point(head)
            .expect("目标显示点应可映射");
        let viewport_top = editor.scroll_anchor().row().get();
        // 导航语义：目标行固定在视口顶部下方 4 行（NAVIGATION_TOP_OFFSET）。
        assert_eq!(
            point.row().get().saturating_sub(viewport_top),
            4,
            "目标行必须按换行后的显示行定位在视口顶部下方：目标显示行 {}，视口顶部 {}",
            point.row().get(),
            viewport_top
        );
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
        let display = editor.display_snapshot(cx);
        editor
            .scroll_manager
            .scroll_by(point(px(100_000.), px(0.)), &display);
        editor.set_selections(SelectionSet::caret(MultiBufferOffset::new(text.len())), cx);
        editor.request_autoscroll(cx);
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
    cx.read_entity(&editor, |editor, cx| {
        assert_eq!(
            editor.scrollbar_thumb_state(),
            ScrollbarThumbState::Dragging,
            "点击轨道应进入拖动态"
        );
        let scroll_top = editor.scroll_top();
        assert!(scroll_top > Pixels::ZERO, "点击轨道应产生滚动");
        assert!(scroll_top <= editor.max_scroll_top());
        assert_eq!(
            editor.selections(cx).primary().head(),
            MultiBufferOffset::ZERO,
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
    let source = buffer.clone();
    inject_editor_diff(
        &editor,
        &source,
        vec![DisplayHunk {
            range: 50..51,
            old_range: 50..51,
            kind: DiffHunkKind::Modified,
            staging: DiffHunkStaging::NoStaging,
        }],
        None,
        cx,
    );
    cx.run_until_parked();

    let (track_bounds, thumb_bounds, per_pixel) = scrollbar_geometry(&editor, cx);
    let thumb_bounds = thumb_bounds.expect("内容超视口时应有 thumb");
    // marker 的轨道位置（绝对定位：行 50 在文档中的位置）。
    let markers = marker_geometry(
        [(
            50..51,
            ScrollbarMarkerKind::Git(EditorHunkMarkerKind::Diff(DiffHunkKind::Modified)),
        )],
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

#[gpui::test]
fn composite_scroll_and_redraw_leave_content_snapshots_unchanged(cx: &mut TestAppContext) {
    let combined = cx.new(MultiBuffer::empty);
    for file in 0..10 {
        let source = test_buffer(
            cx,
            (0..50)
                .map(|row| format!("{file}:{row} 中文\t{}\n", "long words ".repeat(20)))
                .collect::<String>(),
        );
        source.update(cx, |buffer, cx| {
            buffer.set_file_path(format!("src/file_{file:02}.rs").into(), cx)
        });
        combined.update(cx, |buffer, cx| {
            buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(source, 0..50, cx)], cx)
        });
    }
    let (editor, cx) = cx.add_window_view(move |_, cx| Editor::for_multi_buffer(combined, cx));
    editor.update(cx, |editor, cx| {
        editor.set_soft_wrap_mode(Some(SoftWrap::EditorWidth), cx)
    });
    cx.run_until_parked();
    cx.refresh().unwrap();
    cx.run_until_parked();
    cx.refresh().unwrap();
    let before = cx.read_entity(&editor, |editor, cx| editor.display_snapshot(cx));
    assert!(
        before.line_count() > before.buffer_snapshot().line_count(),
        "夹具必须实际启用软换行"
    );
    for delta in [-120., -8_000., 4_000., -1_000_000., 1_000_000., -240.] {
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(100.), px(100.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
            ..Default::default()
        });
        cx.refresh().unwrap();
        cx.run_until_parked();
        cx.read_entity(&editor, |editor, cx| {
            let after = editor.display_snapshot(cx);
            assert_eq!(
                after.version(),
                before.version(),
                "滚动与绘制不得推进显示投影"
            );
            assert_eq!(
                after.buffer_snapshot().version(),
                before.buffer_snapshot().version()
            );
            assert_eq!(
                after.buffer_snapshot().metadata_version(),
                before.buffer_snapshot().metadata_version()
            );
            assert_eq!(editor.file_buffer_ids(cx).len(), 10);
            assert!(editor.has_expanded_buffers(cx));
            assert!(!editor.is_dirty(cx));
        });
    }
}

#[gpui::test]
#[ignore = "手动测量滚轮输入到组合文档窗口绘制完成的耗时"]
fn composite_scroll_input_to_painted_content_latency_probe(cx: &mut TestAppContext) {
    use std::time::Instant;
    use zcv_buffer_diff::{BufferDiff, BufferDiffInput};
    use zcv_language::LanguageRegistry;
    use zcv_multi_buffer::{DiffExcerptRanges, DiffFile};
    for (file_count, staged) in [
        (10, true),
        (10, false),
        (100, true),
        (100, false),
        (300, true),
        (300, false),
    ] {
        let multi = cx.new(if staged {
            MultiBuffer::empty_read_only
        } else {
            MultiBuffer::empty
        });
        let registry = Arc::new(LanguageRegistry::new());
        let base = (0..50)
            .map(|row| format!("old\t{row} {}\n", "中文 abcdefghij ".repeat(20)))
            .collect::<String>();
        let working = base.replace("old", "new");
        let files = (0..file_count)
            .map(|file| {
                let path = std::path::PathBuf::from(format!("src/file_{file:03}.rs"));
                let source = test_buffer(cx, working.clone());
                source.update(cx, |source, cx| source.set_file_path(path.clone(), cx));
                let diff = cx.new(|cx| {
                    BufferDiff::new(
                        BufferDiffInput {
                            working: source,
                            path: path.clone(),
                            base_text: Some(Arc::from(base.clone())),
                            index_text: Some(Arc::from(if staged {
                                working.clone()
                            } else {
                                base.clone()
                            })),
                            language_registry: registry.clone(),
                            key: file as u64,
                            operations: None,
                        },
                        cx,
                    )
                });
                DiffFile {
                    diff,
                    display_path: path,
                    excerpt_ranges: DiffExcerptRanges::FullFile,
                }
            })
            .collect();
        multi.update(cx, |buffer, cx| {
            buffer.set_diff_hunks_expanded_by_default(true, cx);
            buffer.set_diff_files(files, cx);
        });
        cx.run_until_parked();
        let (editor, visual) = cx.add_window_view(move |_, cx| Editor::for_multi_buffer(multi, cx));
        editor.update(&mut *visual, |editor, cx| {
            editor.set_soft_wrap_mode(Some(SoftWrap::EditorWidth), cx)
        });
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let version = visual.read_entity(&editor, |editor, cx| {
            let snapshot = editor.display_snapshot(cx);
            assert!(snapshot.line_count() > snapshot.buffer_snapshot().line_count());
            snapshot.version()
        });
        let direct_started = Instant::now();
        editor.update(&mut *visual, |editor, cx| {
            assert!(editor.scroll_by(point(px(0.), px(-8_000.)), cx));
        });
        let direct_command_ms = direct_started.elapsed().as_secs_f64() * 1_000.;
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let direct_ms = direct_started.elapsed().as_secs_f64() * 1_000.;
        visual.read_entity(&editor, |editor, _| {
            assert!(editor.scroll_top() > Pixels::ZERO, "直接滚动必须改变位置");
            assert!(editor.input_layout.is_some(), "直接滚动必须完成文本绘制");
        });
        editor.update(&mut *visual, |editor, cx| {
            editor.scroll_to(Pixels::ZERO, cx);
        });
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let before_top = visual.read_entity(&editor, |editor, _| editor.scroll_top());
        let first_started = Instant::now();
        visual.simulate_event(ScrollWheelEvent {
            position: point(px(300.), px(300.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-8_000.))),
            ..Default::default()
        });
        let first_event_ms = first_started.elapsed().as_secs_f64() * 1_000.;
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let first_ms = first_started.elapsed().as_secs_f64() * 1_000.;
        visual.read_entity(&editor, |editor, _| {
            assert_ne!(editor.scroll_top(), before_top, "首次滚动必须改变位置");
            assert!(editor.input_layout.is_some(), "首次滚动必须完成文本绘制");
        });
        editor.update(&mut *visual, |editor, cx| {
            editor.scroll_to(Pixels::ZERO, cx);
        });
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let mut samples = Vec::new();
        for frame in 0..240 {
            let delta = [-120., -1_200., 120., 1_200.][frame % 4];
            let before_top = visual.read_entity(&editor, |editor, _| editor.scroll_top());
            let started = Instant::now();
            visual.simulate_event(ScrollWheelEvent {
                position: point(px(300.), px(300.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                ..Default::default()
            });
            visual.update(|window, cx| {
                window.draw(cx).clear(cx);
            });
            samples.push(started.elapsed().as_secs_f64() * 1_000.);
            visual.read_entity(&editor, |editor, _| {
                assert_ne!(
                    editor.scroll_top(),
                    before_top,
                    "第 {frame} 次滚动没有改变位置，滚动量={delta}"
                );
                assert!(editor.input_layout.is_some(), "滚动后必须完成文本绘制");
            });
        }
        let final_version =
            visual.read_entity(&editor, |editor, cx| editor.display_snapshot(cx).version());
        samples.sort_by(f64::total_cmp);
        println!(
            "软换行 diff，暂存={staged}，文件={file_count}，变更行={}；直接命令到绘制={direct_ms:.3} 毫秒（命令={direct_command_ms:.3}），模拟事件到绘制={first_ms:.3} 毫秒（派发及任务排空={first_event_ms:.3}，显式绘制={:.3}）；后续中位数={:.3}，P95={:.3}，范围={:.3}..{:.3}；显示投影版本={version}→{final_version}（GPUI 测试文本系统）",
            file_count * 50,
            first_ms - first_event_ms,
            samples[120],
            samples[228],
            samples[0],
            samples[239]
        );
    }
}

#[gpui::test]
fn batch_file_folding_publishes_one_semantic_change(cx: &mut TestAppContext) {
    let combined = cx.new(MultiBuffer::empty);
    for file in 0..10 {
        let source = test_buffer(cx, "one\ntwo\n".to_owned());
        source.update(cx, |source, cx| {
            source.set_file_path(format!("src/{file}.rs").into(), cx)
        });
        combined.update(cx, |buffer, cx| {
            buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(source, 0..2, cx)], cx)
        });
    }
    let editor = cx.new(|cx| Editor::for_multi_buffer(combined, cx));
    let events = std::rc::Rc::new(std::cell::Cell::new(0));
    cx.run_until_parked();
    let observed = events.clone();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&editor, move |_, event: &EditorEvent, _| {
            if matches!(event, EditorEvent::BufferFoldChanged) {
                observed.set(observed.get() + 1);
            }
        })
    });
    let ids = cx.read_entity(&editor, |editor, cx| editor.file_buffer_ids(cx));
    let before = cx.read_entity(&editor, |editor, cx| editor.display_snapshot(cx));
    editor.update(cx, |editor, cx| {
        editor.set_buffers_folded(ids.clone(), true, cx)
    });
    cx.run_until_parked();
    assert_eq!(events.get(), 1);
    let folded = cx.read_entity(&editor, |editor, cx| editor.display_snapshot(cx));
    assert_eq!(folded.version(), before.version() + 1);
    assert_eq!(
        folded.buffer_snapshot().version(),
        before.buffer_snapshot().version()
    );
    assert!(!cx.read_entity(&editor, |editor, cx| editor.has_expanded_buffers(cx)));
    assert!(
        cx.read_entity(&editor, |editor, _| editor
            .scrollbar_marker_state
            .should_refresh(Default::default())),
        "显示配置提交后必须失效滚动条几何"
    );
    editor.update(cx, |editor, cx| {
        editor.set_buffers_folded(ids.clone(), true, cx)
    });
    cx.run_until_parked();
    assert_eq!(events.get(), 1, "未变化策略不得重复发布");
    assert_eq!(
        cx.read_entity(&editor, |editor, cx| editor.display_snapshot(cx).version()),
        folded.version()
    );
    editor.update(cx, |editor, cx| editor.set_buffers_folded(ids, false, cx));
    cx.run_until_parked();
    assert_eq!(events.get(), 2);
    assert!(cx.read_entity(&editor, |editor, cx| editor.has_expanded_buffers(cx)));
}

#[gpui::test]
fn removing_then_unfolding_a_file_keeps_empty_frames_stable(cx: &mut TestAppContext) {
    let source = test_buffer(cx, "line\n".to_owned());
    source.update(cx, |source, cx| source.set_file_path("src/a.rs".into(), cx));
    let multi = cx.new(MultiBuffer::empty);
    multi.update(cx, |buffer, cx| {
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(source, 0..1, cx)], cx)
    });
    let editor = cx.new(|cx| Editor::for_multi_buffer(multi.clone(), cx));
    cx.run_until_parked();
    let ids = cx.read_entity(&editor, |editor, cx| editor.file_buffer_ids(cx));
    editor.update(cx, |editor, cx| {
        editor.set_buffers_folded(ids.clone(), true, cx)
    });
    multi.update(cx, |buffer, cx| {
        buffer.remove_excerpts_for_path(std::path::Path::new("src/a.rs"), cx)
    });
    cx.run_until_parked();
    editor.update(cx, |editor, cx| editor.set_buffers_folded(ids, false, cx));
    cx.run_until_parked();
    let version = cx.read_entity(&editor, |editor, cx| editor.display_snapshot(cx).version());
    for _ in 0..10 {
        editor.update(cx, |editor, cx| editor.advance_snapshots(cx));
        assert_eq!(
            cx.read_entity(&editor, |editor, cx| editor.display_snapshot(cx).version()),
            version,
            "不可见文件的策略更新后空帧必须收敛"
        );
    }
}

#[gpui::test]
fn scrolling_does_not_invalidate_outline_but_editing_does(cx: &mut TestAppContext) {
    let text = (0..120)
        .map(|row| format!("fn item_{row}() {{}}\n"))
        .collect::<String>();
    let source = test_buffer(cx, text);
    let (editor, cx) = cx.add_window_view(move |_, cx| Editor::for_language_buffer(source, cx));
    cx.run_until_parked();
    cx.refresh().expect("测试窗口应可刷新");

    let document_changes = std::rc::Rc::new(std::cell::Cell::new(0usize));
    let observed = document_changes.clone();
    let _document_subscription = cx.cx.update(|cx| {
        cx.subscribe(&editor, move |_, event: &EditorEvent, _| {
            if matches!(event, EditorEvent::DocumentChanged) {
                observed.set(observed.get() + 1);
            }
        })
    });
    document_changes.set(0);

    let before = cx.read_entity(&editor, |editor, cx| editor.outline_version(cx));
    for delta in [-120., -8_000., 4_000., -1_000_000.] {
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(100.), px(100.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
            ..Default::default()
        });
        cx.refresh().expect("测试窗口应可刷新");
        cx.run_until_parked();
        let after = cx.read_entity(&editor, |editor, cx| editor.outline_version(cx));
        assert_eq!(after, before, "滚动不得改变大纲失效键");
    }
    assert_eq!(document_changes.get(), 0, "滚动不得发布文档推进事件");

    cx.update_entity(&editor, |editor, cx| {
        editor.set_text("fn replaced() {}\n", cx);
    });
    cx.run_until_parked();
    let edited = cx.read_entity(&editor, |editor, cx| editor.outline_version(cx));
    assert_ne!(edited, before, "编辑必须推进大纲失效键");
    assert!(document_changes.get() >= 1, "编辑必须发布文档推进事件");
}
