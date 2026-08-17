//! 鼠标选区手势测试：双击选词、三击选行、拖拽扩展（词/行粒度）、Shift+点击按上次粒度扩展。
//!
//! 主路径直接调用 Editor 的 begin/update/end selection（精确控制字节偏移），
//! 另有两个事件级冒烟测试验证 element.rs 的事件接线。

use gpui::{Modifiers, MouseButton, MouseDownEvent, TestAppContext, point, px};
use zcv_engine::{ByteOffset, Selection, SelectionSet, TextRange};

use super::common::{buffer_text, test_buffer};
use super::*;

/// 字节偏移构造辅助（测试文本均为 ASCII，字节数即字符数）。
fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn selections(selection: Selection) -> SelectionSet {
    SelectionSet::new(vec![selection])
}

#[gpui::test]
fn single_click_places_a_caret(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "hello world");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_buffer(buffer, cx)
    });
    cx.run_until_parked();

    editor.update(cx, |editor, cx| editor.begin_selection(b(5), 1, false, cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::caret(b(5))));
    });
}

#[gpui::test]
fn double_click_selects_the_whole_word(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "foo_bar baz\n");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_buffer(buffer, cx)
    });
    cx.run_until_parked();

    // 双击词内部：选中整个标识符（下划线属于词字符）。
    editor.update(cx, |editor, cx| editor.begin_selection(b(3), 2, false, cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::new(b(0), b(7))));
    });
    editor.update(cx, |editor, _| editor.end_selection());

    // 双击词尾（紧贴空格）：仍选中整个词。
    editor.update(cx, |editor, cx| editor.begin_selection(b(7), 2, false, cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::new(b(0), b(7))));
    });
}

#[gpui::test]
fn triple_click_selects_the_whole_line(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "first line\nsecond line\n");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_buffer(buffer, cx)
    });
    cx.run_until_parked();

    // 三击第一行：整行含行尾换行符（first line 共 10 字符 + \n）。
    editor.update(cx, |editor, cx| editor.begin_selection(b(5), 3, false, cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::new(b(0), b(11))));
    });

    // 三击第二行：11..23。
    editor.update(cx, |editor, cx| editor.begin_selection(b(13), 3, false, cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(
            editor.selections(),
            selections(Selection::new(b(11), b(23)))
        );
    });
}

#[gpui::test]
fn quadruple_click_selects_all(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "one two three");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_buffer(buffer, cx)
    });
    cx.run_until_parked();

    editor.update(cx, |editor, cx| editor.begin_selection(b(5), 4, false, cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::new(b(0), b(13))));
    });
}

#[gpui::test]
fn dragging_with_character_granularity_selects_a_range(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "hello world");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_buffer(buffer, cx)
    });
    cx.run_until_parked();

    // 单击后向右拖：anchor 固定在按下点，head 跟随鼠标。
    editor.update(cx, |editor, cx| editor.begin_selection(b(0), 1, false, cx));
    editor.update(cx, |editor, cx| editor.update_selection(b(5), cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::new(b(0), b(5))));
    });
    // 反向拖回：选区收缩，anchor 不变。
    editor.update(cx, |editor, cx| editor.update_selection(b(2), cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::new(b(0), b(2))));
    });
    // 松开后选区保持。
    editor.update(cx, |editor, _| editor.end_selection());
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::new(b(0), b(2))));
    });
}

#[gpui::test]
fn editing_cancels_a_pending_mouse_selection(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "abcdef");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_buffer(buffer, cx)
    });
    cx.run_until_parked();

    cx.update(|window, cx| window.focus(&editor.read(cx).focus_handle()));
    editor.update(cx, |editor, cx| editor.begin_selection(b(1), 1, false, cx));
    editor.update(cx, |editor, cx| editor.update_selection(b(4), cx));
    cx.dispatch_action(Backspace);

    assert_eq!(buffer_text(&buffer, cx), "aef");
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::caret(b(1))));
    });

    // 即使 MouseUp 遗失，编辑后迟到的 dragging MouseMove 也不能用旧锚点复活选区。
    editor.update(cx, |editor, cx| editor.update_selection(b(3), cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::caret(b(1))));
    });
}

#[gpui::test]
fn double_click_drag_selects_whole_words(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "one two three");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_buffer(buffer, cx)
    });
    cx.run_until_parked();

    // 双击 "one"：选中 0..3。
    editor.update(cx, |editor, cx| editor.begin_selection(b(1), 2, false, cx));
    // 拖到 "two" 内部：整词吸附，选区扩展到 0..7（含空格）。
    editor.update(cx, |editor, cx| editor.update_selection(b(5), cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::new(b(0), b(7))));
    });
    // 拖到 "three" 内部：0..13。
    editor.update(cx, |editor, cx| editor.update_selection(b(11), cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::new(b(0), b(13))));
    });
    // 反向拖回 "two"：选区收缩到词尾边界。
    editor.update(cx, |editor, cx| editor.update_selection(b(6), cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::new(b(0), b(7))));
    });
}

#[gpui::test]
fn triple_click_drag_selects_whole_lines(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "aaa\nbbb\nccc\n");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_buffer(buffer, cx)
    });
    cx.run_until_parked();

    // 三击第一行：0..4（含换行）。
    editor.update(cx, |editor, cx| editor.begin_selection(b(0), 3, false, cx));
    // 拖到 "bbb"：整行纳入，0..8。
    editor.update(cx, |editor, cx| editor.update_selection(b(6), cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::new(b(0), b(8))));
    });
    // 拖到 "ccc"：0..12。
    editor.update(cx, |editor, cx| editor.update_selection(b(9), cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::new(b(0), b(12))));
    });
}

#[gpui::test]
fn dragging_leftwards_anchors_against_the_original_word_end(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "one two three");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_buffer(buffer, cx)
    });
    cx.run_until_parked();

    // 双击 "three"：8..13，anchor 在词首。
    editor.update(cx, |editor, cx| editor.begin_selection(b(10), 2, false, cx));
    // 向左拖到 "two"：选区锚定原词右端 13，head 在 "two" 词首 4。
    editor.update(cx, |editor, cx| editor.update_selection(b(5), cx));
    cx.read_entity(&editor, |editor, _| {
        let selections = editor.selections();
        let selection = selections.primary();
        assert_eq!(selection.range(), TextRange::new(b(4), b(13)).unwrap());
        assert_eq!(selection.anchor(), b(13));
        assert_eq!(selection.head(), b(4));
    });
}

#[gpui::test]
fn shift_click_extends_selection_from_the_anchor(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "one two three");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_buffer(buffer, cx)
    });
    cx.run_until_parked();

    // 单击 "one" 内部：caret 1。
    editor.update(cx, |editor, cx| editor.begin_selection(b(1), 1, false, cx));
    // Shift+单击 "three" 内部：从锚点 1 扩展到 10。
    editor.update(cx, |editor, cx| editor.begin_selection(b(10), 1, true, cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::new(b(1), b(10))));
    });
    // 再 Shift+单击回中间：head 移回，选区收缩。
    editor.update(cx, |editor, cx| editor.begin_selection(b(5), 1, true, cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::new(b(1), b(5))));
    });
}

#[gpui::test]
fn shift_double_click_extends_by_word_granularity(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "one two three");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_buffer(buffer, cx)
    });
    cx.run_until_parked();

    // 双击 "one"：0..3，记住词粒度。
    editor.update(cx, |editor, cx| editor.begin_selection(b(1), 2, false, cx));
    // Shift+双击 "three"：按上次词粒度扩展，0..13。
    editor.update(cx, |editor, cx| editor.begin_selection(b(11), 2, true, cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::new(b(0), b(13))));
    });
    // 词粒度仍在：Shift+单击 "two" 也按整词扩展。
    editor.update(cx, |editor, cx| editor.begin_selection(b(5), 1, true, cx));
    cx.read_entity(&editor, |editor, _| {
        assert_eq!(editor.selections(), selections(Selection::new(b(0), b(7))));
    });
}

#[gpui::test]
fn mouse_events_drive_double_click_through_the_element(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "aaaaaa bbbbbb\ncccccc dddddd\n");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_buffer(buffer, cx)
    });
    cx.run_until_parked();
    cx.refresh().expect("测试窗口应可刷新");

    // 文本区在 gutter 右侧：点击一个安全坐标，先单击读回命中列，
    // 规避对 gutter 宽度与字宽的几何依赖。
    let click = point(px(120.), px(2.));
    cx.simulate_event(MouseDownEvent {
        position: click,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 1,
        first_mouse: false,
    });
    cx.run_until_parked();
    let clicked_offset = cx.read_entity(&editor, |editor, _| editor.selections().primary().head());
    assert!(clicked_offset > b(0), "点击应命中文本区而非 gutter");
    let clicked_column = cx.read_entity(&editor, |editor, _| {
        editor
            .render_snapshot()
            .byte_to_position(clicked_offset)
            .expect("光标应有效")
            .column()
            .get()
    });

    // 同一位置双击：选中光标所在词（行 0 为 "aaaaaa bbbbbb"，两 6 字符词）。
    cx.simulate_event(MouseDownEvent {
        position: click,
        modifiers: Modifiers::default(),
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    cx.run_until_parked();
    cx.read_entity(&editor, |editor, _| {
        let (expected_start, expected_end) = match clicked_column {
            0..=5 => (0, 6),
            6 => (6, 7),
            _ => (7, 13),
        };
        let selections = editor.selections();
        let selection = selections.primary();
        assert_eq!(
            selection.range(),
            TextRange::new(b(expected_start), b(expected_end)).unwrap(),
            "双击应选中命中列所在词"
        );
    });
}

#[gpui::test]
fn mouse_dragging_expands_selection_across_rows(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "aaaaaa bbbbbb\ncccccc dddddd\n");
    let (editor, cx) = cx.add_window_view({
        let buffer = buffer.clone();
        move |_, cx| Editor::for_buffer(buffer, cx)
    });
    cx.run_until_parked();
    cx.refresh().expect("测试窗口应可刷新");

    // 单击行 0，再按住左键拖到行 1：选区跨行。
    let click = point(px(120.), px(2.));
    cx.simulate_mouse_down(click, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    let line_height = cx.read_entity(&editor, |editor, _| {
        editor.last_line_height.expect("渲染后应有行高")
    });
    let drag_to = point(click.x, px(2.) + line_height);
    cx.simulate_mouse_move(drag_to, Some(MouseButton::Left), Modifiers::default());
    cx.run_until_parked();
    cx.read_entity(&editor, |editor, _| {
        let selections = editor.selections();
        let selection = selections.primary();
        let range = selection.range();
        assert!(range.start() < range.end(), "拖拽应产生非空选区");
        let start_row = editor
            .render_snapshot()
            .byte_to_position(range.start())
            .expect("选区起点应有效")
            .line()
            .get();
        let end_row = editor
            .render_snapshot()
            .byte_to_position(range.end())
            .expect("选区终点应有效")
            .line()
            .get();
        assert!(start_row < end_row, "拖拽跨行应产生跨行选区");
    });

    // 松开：选区保持。
    cx.simulate_mouse_up(drag_to, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    cx.read_entity(&editor, |editor, _| {
        let range = editor.selections().primary().range();
        assert!(range.start() < range.end());
    });
}
