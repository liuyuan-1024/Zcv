use std::time::{Duration, Instant};

use super::*;
use crate::{
    TerminalView,
    test::support::{display_only_terminal, write_output},
};
use gpui::{
    ClipboardItem, Context, Entity, EntityInputHandler, IntoElement, KeyBinding, Modifiers,
    MouseButton, Render, TestAppContext, VisualTestContext, Window, div, point, prelude::*, px,
    size,
};
use zcv_actions::{Copy, Interrupt, Paste};
use zcv_settings::{SettingsStore, UserSettings};

#[derive(Default)]
struct EmptyView;

impl Render for EmptyView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn build_terminal(cx: &mut TestAppContext) -> Entity<Terminal> {
    cx.new(|cx| display_only_terminal(&TerminalBuilder::new(), cx))
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
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let terminal = build_terminal(cx);
    let (_, cx) = cx.add_window_view(|_window, _cx| EmptyView);

    cx.update(|_window, cx| {
        terminal.update(cx, |t, cx| {
            write_output(t, b"zcv-terminal-ok\r\n", cx);
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
    cx.set_global(SettingsStore::new(UserSettings::default()));
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
    cx.set_global(SettingsStore::new(UserSettings::default()));
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

/// 焦点先于首次挂载到达视图时，首次注册监听仍要同步到终端状态机。
#[gpui::test]
async fn focus_before_first_render_is_synchronized_to_terminal(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let terminal = build_terminal(cx);
    let terminal_for_view = terminal.clone();
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let view = TerminalView::new(terminal_for_view, cx);
        let focus = view.focus_handle();
        window.focus(&focus, cx);
        view
    });
    cx.run_until_parked();

    assert!(
        cx.read_entity(&terminal, |terminal, _| terminal.term.lock().is_focused),
        "终端状态机应从首次挂载时的权威焦点句柄同步聚焦状态"
    );
}

/// 光标与焦点绑定：失焦隐藏；聚焦时（终端未表态）默认闪烁可见。
#[gpui::test]
async fn cursor_focus_binding(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
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

/// 渲染冒烟：display-only 终端渲染一帧不 panic，输入写入不依赖 PTY。
#[gpui::test]
async fn render_smoke(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
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
    cx.set_global(SettingsStore::new(UserSettings::default()));
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
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let terminal = build_terminal(cx);
    let (_, cx) = cx.add_window_view(|_window, _cx| EmptyView);

    let started = cx.update(|_window, cx| {
        terminal.update(cx, |terminal, cx| {
            let event = gpui::MouseDownEvent {
                button: MouseButton::Left,
                position: point(px(16.), px(16.)),
                modifiers: Modifiers::default(),
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
    cx.set_global(SettingsStore::new(UserSettings::default()));
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

    cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
    cx.refresh().expect("按下后应能刷新选择锚点");
    cx.simulate_mouse_move(outside, Some(MouseButton::Left), Modifiers::default());
    cx.refresh().expect("拖出视口后应能刷新选择");
    let selection_before_release = cx
        .read_entity(&terminal, |terminal, _| terminal.last_content().selection)
        .expect("拖拽后应存在选择范围");

    cx.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    assert!(
        !cx.read_entity(&terminal, |terminal, _| terminal.selection_started()),
        "视口外释放也应结束终端指针手势"
    );

    cx.simulate_mouse_move(return_to_terminal, None, Modifiers::default());
    cx.refresh().expect("无按键移动后应能刷新");
    let selection_after_return = cx
        .read_entity(&terminal, |terminal, _| terminal.last_content().selection)
        .expect("释放后选择范围应保留以供复制");
    assert_eq!(
        selection_after_return, selection_before_release,
        "释放后无按键移动不得继续扩展选择"
    );
}

/// 绑定与内置 keymap 相同的两组 ctrl-c：Terminal 中断、有选区时复制。
fn bind_ctrl_c_conflict(cx: &mut TestAppContext) {
    cx.update(|app| {
        app.bind_keys([
            KeyBinding::new("ctrl-c", Interrupt, Some("Terminal")),
            KeyBinding::new("ctrl-c", Copy, Some("Terminal && selection")),
        ]);
    });
}

/// 有选区时 ctrl-c 应命中 `Terminal && selection` 的复制绑定：
/// 剪贴板收到选中文本，且不回落到 Interrupt（其用户输入语义会清除选区）。
#[gpui::test]
async fn ctrl_c_copies_selection_instead_of_interrupting(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    bind_ctrl_c_conflict(cx);
    let terminal = build_terminal(cx);
    let terminal_for_view = terminal.clone();
    let (view, cx) =
        cx.add_window_view(move |_window, cx| TerminalView::new(terminal_for_view, cx));
    cx.run_until_parked();

    cx.update(|_window, cx| {
        terminal.update(cx, |t, cx| {
            write_output(t, b"hello terminal\r\n", cx);
            t.select_range(
                SelectionType::Simple,
                SelectionPoint {
                    point: Point { line: 0, column: 0 },
                    side: SelectionSide::Left,
                },
                Some(SelectionPoint {
                    point: Point { line: 0, column: 5 },
                    side: SelectionSide::Left,
                }),
                cx,
            );
            t.sync(cx);
        });
    });
    let selected = cx.update(|_window, cx| terminal.read(cx).last_content().selection_text.clone());
    assert_eq!(selected.as_deref(), Some("hello"), "应选中 hello");

    cx.update(|window, cx| window.focus(&view.read(cx).focus_handle(), cx));
    cx.refresh().expect("建立选区后应能刷新");
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();

    let (clipboard, has_selection) = cx.update(|_window, cx| {
        (
            cx.read_from_clipboard().and_then(|item| item.text()),
            terminal.update(cx, |t, cx| {
                t.sync(cx);
                t.last_content().selection.is_some()
            }),
        )
    });
    assert_eq!(
        clipboard.as_deref(),
        Some("hello"),
        "有选区时 ctrl-c 应把选区复制到剪贴板"
    );
    assert!(
        has_selection,
        "复制不应清除选区；选区消失说明 Interrupt 也被分发"
    );
}

/// ctrl-v 应把剪贴板内容粘贴进终端。
///
/// 粘贴走 write_input（滚回底部），且仅在剪贴板有内容时发生；
/// 空剪贴板阶段用于排除其他按键动作造成相同副作用的假阳性。
#[gpui::test]
async fn ctrl_v_pastes_clipboard_into_terminal(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    cx.update(|app| {
        app.bind_keys([KeyBinding::new("ctrl-v", Paste, Some("Terminal"))]);
    });
    let terminal = build_terminal(cx);
    let terminal_for_view = terminal.clone();
    let (view, cx) =
        cx.add_window_view(move |_window, cx| TerminalView::new(terminal_for_view, cx));
    cx.run_until_parked();

    // 写入远超一屏的输出并向上回看，制造非底部状态。
    cx.update(|_window, cx| {
        terminal.update(cx, |t, cx| {
            let mut output = Vec::new();
            for i in 0..500 {
                output.extend_from_slice(format!("line-{i:03}\r\n").as_bytes());
            }
            write_output(t, &output, cx);
            t.sync(cx);
            t.scroll_lines(3, cx);
            t.sync(cx);
        });
    });
    let offset = cx.update(|_window, cx| terminal.read(cx).last_content().display_offset);
    assert!(offset > 0, "按键前应已滚离底部");

    cx.update(|window, cx| window.focus(&view.read(cx).focus_handle(), cx));
    cx.refresh().expect("聚焦后应能刷新");

    // 阶段一：剪贴板为空，粘贴不应产生用户输入副作用。
    cx.simulate_keystrokes("ctrl-v");
    cx.update(|_window, cx| {
        terminal.update(cx, |t, cx| t.sync(cx));
    });
    let offset = cx.update(|_window, cx| terminal.read(cx).last_content().display_offset);
    assert!(offset > 0, "空剪贴板时粘贴不应改变视口");

    // 阶段二：剪贴板有内容，粘贴把它作为用户输入写入（滚回底部）。
    cx.update(|_window, cx| {
        cx.write_to_clipboard(ClipboardItem::new_string("pasted text".to_string()));
    });
    cx.simulate_keystrokes("ctrl-v");
    cx.update(|_window, cx| {
        terminal.update(cx, |t, cx| t.sync(cx));
    });
    let offset = cx.update(|_window, cx| terminal.read(cx).last_content().display_offset);
    assert_eq!(offset, 0, "粘贴应把视口带回底部");
}

/// 无选区时 ctrl-c 应回落到 Terminal 上下文的 Interrupt：
/// 用户输入语义生效（滚回底部），且不写剪贴板。
#[gpui::test]
async fn ctrl_c_interrupts_without_selection(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    bind_ctrl_c_conflict(cx);
    let terminal = build_terminal(cx);
    let terminal_for_view = terminal.clone();
    let (view, cx) =
        cx.add_window_view(move |_window, cx| TerminalView::new(terminal_for_view, cx));
    cx.run_until_parked();

    // 写入远超一屏的输出并向上回看，制造非底部状态；
    // 行数需覆盖任意测试窗口尺寸，保证滚动历史非空。
    cx.update(|_window, cx| {
        terminal.update(cx, |t, cx| {
            let mut output = Vec::new();
            for i in 0..500 {
                output.extend_from_slice(format!("line-{i:03}\r\n").as_bytes());
            }
            write_output(t, &output, cx);
            t.sync(cx);
            t.scroll_lines(3, cx);
            t.sync(cx);
        });
    });
    let offset = cx.update(|_window, cx| terminal.read(cx).last_content().display_offset);
    assert!(offset > 0, "按键前应已滚离底部");

    cx.update(|window, cx| window.focus(&view.read(cx).focus_handle(), cx));
    cx.refresh().expect("滚动后应能刷新");
    cx.simulate_keystrokes("ctrl-c");
    cx.run_until_parked();

    let (offset, clipboard) = cx.update(|_window, cx| {
        terminal.update(cx, |t, cx| t.sync(cx));
        (
            terminal.read(cx).last_content().display_offset,
            cx.read_from_clipboard().and_then(|item| item.text()),
        )
    });
    assert_eq!(
        offset, 0,
        "无选区时 ctrl-c 应分发 Interrupt，其用户输入语义滚回底部"
    );
    assert!(clipboard.is_none(), "无选区时 ctrl-c 不应写剪贴板");
}
