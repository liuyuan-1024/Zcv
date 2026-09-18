use std::time::{Duration, Instant};

use super::*;
use crate::TerminalView;
use gpui::{
    Context, Entity, EntityInputHandler, IntoElement, MouseButton, Render, TestAppContext,
    VisualTestContext, Window, div, point, prelude::*, px, size,
};

#[derive(Default)]
struct EmptyView;

impl Render for EmptyView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn build_terminal(cx: &mut TestAppContext) -> Entity<Terminal> {
    cx.new(|cx| Terminal::new_display_only(&TerminalBuilder::new(), cx))
}

/// 刷新渲染快照并断言内容满足条件。
async fn wait_for_content(
    cx: &mut VisualTestContext,
    terminal: &Entity<Terminal>,
    mut predicate: impl FnMut(&Content) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let matched = cx.update(|_window, cx| {
            terminal.update(cx, |t, cx| t.sync(cx));
            predicate(terminal.read(cx).last_content())
        });
        if matched {
            return;
        }
        assert!(Instant::now() < deadline, "等待终端内容超时");
        cx.background_executor
            .timer(Duration::from_millis(20))
            .await;
        cx.run_until_parked();
    }
}

/// 把快照中全部单元格按顺序拼接为文本（含空格）。
fn all_text(content: &Content) -> String {
    content.cells.iter().map(|ic| ic.cell.character()).collect()
}

/// 模拟器接收输出后应投影到渲染快照。
#[gpui::test]
async fn terminal_output_updates_content(cx: &mut TestAppContext) {
    let terminal = build_terminal(cx);
    let (_, cx) = cx.add_window_view(|_window, _cx| EmptyView);

    cx.update(|_window, cx| {
        terminal.update(cx, |t, cx| {
            t.write_output(b"zcv-terminal-ok\r\n", cx);
        });
    });

    wait_for_content(cx, &terminal, |content| {
        all_text(content).contains("zcv-terminal-ok")
    })
    .await;
}

/// 像素高度变化时，模型仍需立即持有最新边界，不能等待下一次 PTY resize。
#[gpui::test]
async fn terminal_size_snapshot_is_authoritative_before_resize(cx: &mut TestAppContext) {
    let terminal = build_terminal(cx);
    let (_, cx) = cx.add_window_view(|_window, _cx| EmptyView);

    let first_bounds = TerminalBounds::new(px(8.), px(16.), size(px(100.), px(100.)));
    let pixel_changed_bounds = TerminalBounds::new(px(8.), px(16.), size(px(101.), px(101.)));

    cx.update(|_window, cx| {
        terminal.update(cx, |terminal, cx| {
            terminal.set_size(first_bounds, cx);
            terminal.sync(cx);
            terminal.set_size(pixel_changed_bounds, cx);
            terminal.sync(cx);
        });
    });

    let actual_bounds = cx.update(|_window, cx| terminal.read(cx).last_content().terminal_bounds);
    assert_eq!(actual_bounds, pixel_changed_bounds);
}

/// IME 候选窗定位：渲染一帧后 bounds_for_range 应返回光标像素位置。
#[gpui::test]
async fn ime_cursor_bounds(cx: &mut TestAppContext) {
    let terminal = build_terminal(cx);
    let (view, cx) = cx.add_window_view(|_window, cx| TerminalView::new(terminal, cx));
    cx.run_until_parked();
    cx.background_executor
        .timer(Duration::from_millis(100))
        .await;
    cx.run_until_parked();

    let bounds = cx.update(|window, cx| {
        view.update(cx, |view, cx| {
            view.bounds_for_range(0..0, window.bounds(), window, cx)
        })
    });
    assert!(bounds.is_some(), "渲染后候选窗位置应有效");
    let bounds = bounds.unwrap();
    assert!(f32::from(bounds.origin.x) >= 0.);
    assert!(f32::from(bounds.origin.y) >= 0.);
}

/// 光标与焦点绑定：失焦隐藏；聚焦时（终端未表态）默认闪烁可见。
#[gpui::test]
async fn cursor_focus_binding(cx: &mut TestAppContext) {
    let terminal = build_terminal(cx);
    let (view, cx) = cx.add_window_view(|_window, cx| TerminalView::new(terminal, cx));
    cx.run_until_parked();

    let (unfocused, focused) = cx.update(|_window, cx| {
        (
            view.read(cx).should_show_cursor(false, cx),
            view.read(cx).should_show_cursor(true, cx),
        )
    });
    assert!(!unfocused, "失焦时不应显示光标");
    assert!(focused, "聚焦时应显示光标");
}

/// 渲染冒烟：真实 PTY 终端 + 视图渲染一帧不 panic。
#[gpui::test]
async fn render_smoke(cx: &mut TestAppContext) {
    let terminal = build_terminal(cx);
    let (view, cx) = cx.add_window_view(|_window, cx| TerminalView::new(terminal, cx));
    cx.run_until_parked();
    cx.background_executor
        .timer(Duration::from_millis(100))
        .await;
    cx.run_until_parked();
    // 输入后再次渲染。
    cx.update(|_window, cx| {
        view.update(cx, |view, cx| {
            view.terminal.update(cx, |t, cx| {
                t.write_input(b"echo rendered\n".to_vec(), cx);
            });
        });
    });
    cx.run_until_parked();
}

/// 选择设置与清除：select_range 后应报告存在选择，清除后消失。
#[gpui::test]
async fn selection_set_and_clear(cx: &mut TestAppContext) {
    let terminal = build_terminal(cx);
    let (_, cx) = cx.add_window_view(|_window, _cx| EmptyView);

    wait_for_content(cx, &terminal, |content| !content.cells.is_empty()).await;

    let has_selection = cx.update(|window, cx| {
        terminal.update(cx, |t, cx| {
            t.set_size(
                TerminalBounds::new(px(8.), px(16.), window.bounds().size),
                cx,
            );
            // 空网格时有效行从 0 开始（视口首行）；选择需有长度。
            t.select_range(
                SelectionType::Simple,
                SelectionPoint {
                    point: Point { line: 0, column: 0 },
                    side: SelectionSide::Left,
                },
                Some(SelectionPoint {
                    point: Point { line: 0, column: 3 },
                    side: SelectionSide::Right,
                }),
                cx,
            );
            t.sync(cx);
        });
        terminal.read(cx).last_content().selection.is_some()
    });
    assert!(has_selection, "设置选择后内容快照应携带选择");

    let cleared = cx.update(|_window, cx| {
        terminal.update(cx, |t, cx| {
            t.write_input(Vec::new(), cx);
            t.sync(cx);
        });
        terminal.read(cx).last_content().selection.is_none()
    });
    assert!(cleared, "清除选择后内容快照不应再有选择");
}

/// 终端刚创建且尚无 PTY 输出时，鼠标按下也必须能建立选区手势。
#[gpui::test]
async fn selection_starts_before_terminal_output(cx: &mut TestAppContext) {
    let terminal = build_terminal(cx);
    let (_, cx) = cx.add_window_view(|_window, _cx| EmptyView);

    let started = cx.update(|_window, cx| {
        terminal.update(cx, |terminal, cx| {
            let event = gpui::MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(16.), px(16.)),
                modifiers: gpui::Modifiers::default(),
                click_count: 1,
                first_mouse: true,
            };
            let geometry = SelectionGeometry {
                origin: point(px(0.), px(0.)),
                cell_width: px(8.),
                line_height: px(16.),
                screen_lines: 30,
            };
            let started = terminal.mouse_down(&event, geometry, cx);
            terminal.sync(cx);
            started && terminal.selection_started()
        })
    });
    assert!(started, "空终端首次渲染前也应能开始选区");
}

/// 拖拽选择可在视口外释放；释放后移回终端不能继续改写已完成的选择。
#[gpui::test]
async fn releasing_selection_outside_the_terminal_ends_the_pointer_gesture(
    cx: &mut TestAppContext,
) {
    let terminal = build_terminal(cx);
    let terminal_for_view = terminal.clone();
    let (_view, cx) =
        cx.add_window_view(move |_window, cx| TerminalView::new(terminal_for_view, cx));
    cx.run_until_parked();
    cx.refresh().expect("测试窗口应可刷新");

    let (start, outside, return_to_terminal) = cx.update(|window, _| {
        let bounds = window.bounds();
        (
            point(bounds.left() + px(16.), bounds.top() + px(16.)),
            point(bounds.right() + px(160.), bounds.bottom() + px(160.)),
            point(bounds.left() + px(48.), bounds.top() + px(16.)),
        )
    });

    cx.simulate_mouse_down(start, MouseButton::Left, gpui::Modifiers::default());
    cx.refresh().expect("按下后应能刷新选择锚点");
    cx.simulate_mouse_move(outside, Some(MouseButton::Left), gpui::Modifiers::default());
    cx.refresh().expect("拖出视口后应能刷新选择");
    let selection_before_release = cx
        .read_entity(&terminal, |terminal, _| terminal.last_content().selection)
        .expect("拖拽后应存在选择范围");

    cx.simulate_mouse_up(outside, MouseButton::Left, gpui::Modifiers::default());
    cx.run_until_parked();
    assert!(
        !cx.read_entity(&terminal, |terminal, _| terminal.selection_started()),
        "视口外释放也应结束终端指针手势"
    );

    cx.simulate_mouse_move(return_to_terminal, None, gpui::Modifiers::default());
    cx.refresh().expect("无按键移动后应能刷新");
    let selection_after_return = cx
        .read_entity(&terminal, |terminal, _| terminal.last_content().selection)
        .expect("释放后选择范围应保留以供复制");
    assert_eq!(
        selection_after_return, selection_before_release,
        "释放后无按键移动不得继续扩展选择"
    );
}
