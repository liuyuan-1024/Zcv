use std::time::Duration;
use std::{cell::Cell, rc::Rc};

use gpui::{AppContext, TestAppContext};

use super::{ToastAction, ToastKind, ToastLayer};

#[gpui::test]
fn multiple_toasts_keep_independent_actions_and_lifetimes(cx: &mut TestAppContext) {
    let layer = cx.new(|_| ToastLayer::new());
    let (first, second, persistent) = layer.update(cx, |layer, cx| {
        let first = layer.show(
            ToastKind::Info,
            "第一条",
            vec![],
            Some(Duration::from_secs(5)),
            cx,
        );
        let second = layer.show(
            ToastKind::Error,
            "第二条",
            vec![
                ToastAction::new("覆盖", |_, _| {}),
                ToastAction::new("还原", |_, _| {}),
                ToastAction::new("取消", |_, _| {}),
            ],
            None,
            cx,
        );
        let persistent = layer.show(ToastKind::Info, "第三条", vec![], None, cx);
        (first, second, persistent)
    });

    cx.read_entity(&layer, |layer, _| {
        assert_eq!(layer.toasts.len(), 3);
        assert_eq!(layer.toasts[1].actions.len(), 3);
    });

    layer.update(cx, |layer, cx| layer.hide(second, cx));
    cx.read_entity(&layer, |layer, _| {
        assert_eq!(layer.toasts.len(), 2);
        assert_eq!(layer.toasts[0].id, first);
        assert_eq!(layer.toasts[1].id, persistent);
    });

    let other_layer = cx.new(|_| ToastLayer::new());
    let other_id = other_layer.update(cx, |layer, cx| {
        layer.show(ToastKind::Info, "另一个工作区", vec![], None, cx)
    });
    layer.update(cx, |layer, cx| layer.hide(other_id, cx));
    cx.read_entity(&layer, |layer, _| assert_eq!(layer.toasts.len(), 2));
}

#[gpui::test]
fn timed_toasts_expire_independently_and_persistent_toasts_remain(cx: &mut TestAppContext) {
    let layer = cx.new(|_| ToastLayer::new());
    let (first, second, persistent) = layer.update(cx, |layer, cx| {
        let first = layer.show(
            ToastKind::Info,
            "第一条",
            vec![],
            Some(Duration::from_secs(2)),
            cx,
        );
        let second = layer.show(
            ToastKind::Info,
            "第二条",
            vec![],
            Some(Duration::from_secs(1)),
            cx,
        );
        let persistent = layer.show(ToastKind::Error, "常驻", vec![], None, cx);
        (first, second, persistent)
    });

    layer.update(cx, |layer, _| layer.pause_dismiss_timer(first));
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
    cx.read_entity(&layer, |layer, _| {
        assert_eq!(layer.toasts.len(), 2);
        assert_eq!(layer.toasts[0].id, first);
        assert_eq!(layer.toasts[1].id, persistent);
        assert!(!layer.toasts.iter().any(|toast| toast.id == second));
    });

    layer.update(cx, |layer, cx| layer.restart_dismiss_timer(first, cx));
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
    cx.read_entity(&layer, |layer, _| {
        assert_eq!(layer.toasts.len(), 1);
        assert_eq!(layer.toasts[0].id, persistent);
    });
}

#[gpui::test]
fn new_toast_appears_below_and_moves_older_toast_up(cx: &mut TestAppContext) {
    let (layer, cx) = cx.add_window_view(|_, _| ToastLayer::new());
    layer.update(cx, |layer, cx| {
        layer.show(ToastKind::Info, "旧提示", vec![], None, cx);
    });
    let first_frame = cx.debug_bounds("toast-0").expect("首条提示应显示");
    assert!(
        first_frame.size.height > gpui::px(40.0),
        "首条提示不应被裁剪"
    );
    let window_bounds = cx.update(|window, _| window.bounds());
    assert!(
        first_frame.bottom() <= window_bounds.bottom(),
        "首条提示应完整位于窗口内"
    );
    let old_before = cx.debug_bounds("toast-0").expect("旧提示应显示");

    layer.update(cx, |layer, cx| {
        layer.show(ToastKind::Info, "新提示", vec![], None, cx);
    });
    let old_start = cx.debug_bounds("toast-0").expect("旧提示应保持原位置");
    assert!(old_before.top() - old_start.top() < gpui::px(16.0));
    std::thread::sleep(Duration::from_millis(100));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    let old_mid = cx.debug_bounds("toast-0").expect("旧提示应正在上移");
    std::thread::sleep(Duration::from_millis(250));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    let old_after = cx.debug_bounds("toast-0").expect("旧提示仍应显示");
    let new = cx.debug_bounds("toast-1").expect("新提示应显示");
    assert!(old_start.top() > old_after.top(), "旧提示应从下方向上浮动");
    assert!(old_mid.top() < old_start.top(), "旧提示应逐帧上移");
    assert!(old_mid.top() > old_after.top(), "旧提示不应瞬间到达终点");
    assert!(old_after.top() < old_before.top(), "旧提示应被向上推");
    assert!(old_after.bottom() < new.top(), "新提示应位于底部");
}

#[gpui::test]
fn tall_toast_pushes_older_toast_by_its_measured_height(cx: &mut TestAppContext) {
    let (layer, cx) = cx.add_window_view(|_, _| ToastLayer::new());
    layer.update(cx, |layer, cx| {
        layer.show(ToastKind::Info, "旧提示", vec![], None, cx);
    });
    let old_before = cx.debug_bounds("toast-0").expect("旧提示应显示");

    layer.update(cx, |layer, cx| {
        layer.show(
            ToastKind::Error,
            "这条提示有多行内容，需要在固定宽度内自动换行，同时还要容纳多个操作按钮。文件在磁盘上发生了变化，请检查当前编辑内容后再决定如何处理。",
            vec![
                ToastAction::new("覆盖磁盘文件", |_, _| {}),
                ToastAction::new("还原当前内容", |_, _| {}),
                ToastAction::new("稍后处理", |_, _| {}),
            ],
            None,
            cx,
        );
    });
    let old_start = cx.debug_bounds("toast-0").expect("旧提示应保持原位置");
    let tall_start = cx.debug_bounds("toast-1").expect("新提示应显示");
    assert!(tall_start.size.height > old_start.size.height);
    assert!((old_start.top() - old_before.top()).abs() < gpui::px(2.0));

    std::thread::sleep(Duration::from_millis(350));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    let old_after = cx.debug_bounds("toast-0").expect("旧提示仍应显示");
    let tall_after = cx.debug_bounds("toast-1").expect("新提示仍应显示");
    assert!(old_after.top() < old_before.top());
    assert!(old_after.bottom() < tall_after.top());
}

#[gpui::test]
fn rapid_toasts_continue_rising_without_jumping(cx: &mut TestAppContext) {
    let (layer, cx) = cx.add_window_view(|_, _| ToastLayer::new());
    layer.update(cx, |layer, cx| {
        layer.show(ToastKind::Info, "第一条", vec![], None, cx);
    });
    assert!(cx.debug_bounds("toast-0").is_some());
    layer.update(cx, |layer, cx| {
        layer.show(ToastKind::Info, "第二条", vec![], None, cx);
    });
    assert!(cx.debug_bounds("toast-1").is_some());
    std::thread::sleep(Duration::from_millis(100));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    let before_third = cx.debug_bounds("toast-0").expect("第一条应显示");

    layer.update(cx, |layer, cx| {
        layer.show(ToastKind::Info, "第三条", vec![], None, cx);
    });
    let after_third = cx.debug_bounds("toast-0").expect("第一条应继续显示");
    assert!((after_third.top() - before_third.top()).abs() < gpui::px(2.0));
}

#[gpui::test]
fn overflowing_toast_stack_keeps_latest_toast_visible(cx: &mut TestAppContext) {
    let (layer, cx) = cx.add_window_view(|_, _| ToastLayer::new());
    cx.update(|_, cx| cx.set_reduce_motion(true));
    layer.update(cx, |layer, cx| {
        for index in 0..12 {
            layer.show(
                ToastKind::Info,
                format!("第 {index} 条提示"),
                vec![],
                None,
                cx,
            );
        }
    });

    let latest = cx.debug_bounds("toast-11").expect("最新提示应显示");
    let window = cx.update(|window, _| window.bounds());
    assert!(latest.top() >= window.top());
    assert!(latest.bottom() <= window.bottom());
}

#[gpui::test]
fn closing_newest_toast_does_not_replay_entrance(cx: &mut TestAppContext) {
    let (layer, cx) = cx.add_window_view(|_, _| ToastLayer::new());
    let newest = layer.update(cx, |layer, cx| {
        layer.show(ToastKind::Info, "第一条", vec![], None, cx);
        layer.show(ToastKind::Info, "第二条", vec![], None, cx);
        layer.show(ToastKind::Info, "第三条", vec![], None, cx)
    });
    assert!(cx.debug_bounds("toast-0").is_some());

    layer.update(cx, |layer, cx| layer.hide(newest, cx));
    let after_close = cx.debug_bounds("toast-0").expect("已有提示应显示");
    std::thread::sleep(Duration::from_millis(350));
    cx.update(|window, cx| window.simulate_next_frame(cx));
    cx.run_until_parked();
    let after_wait = cx.debug_bounds("toast-0").expect("已有提示仍应显示");
    assert_eq!(
        after_close.top(),
        after_wait.top(),
        "关闭提示不应重播入场动画"
    );
}

#[gpui::test]
fn toast_renders_all_caller_actions_and_closes_only_the_chosen_toast(cx: &mut TestAppContext) {
    let (layer, cx) = cx.add_window_view(|_, _| ToastLayer::new());
    cx.update(|_, cx| cx.set_reduce_motion(true));
    let chosen = Rc::new(Cell::new(None));
    layer.update(cx, |layer, cx| {
        layer.show(ToastKind::Info, "已有提示", vec![], None, cx);
        let actions = ["覆盖", "还原", "取消"]
            .into_iter()
            .enumerate()
            .map(|(index, label)| {
                let chosen = chosen.clone();
                ToastAction::new(label, move |_, _| chosen.set(Some(index)))
            })
            .collect();
        layer.show(ToastKind::Error, "文件已改动", actions, None, cx);
    });
    assert!(cx.debug_bounds("toast-0").is_some());
    cx.run_until_parked();
    let first = cx
        .debug_bounds("toast-action-1-0")
        .expect("第一个按钮应显示");
    let second = cx
        .debug_bounds("toast-action-1-1")
        .expect("第二个按钮应显示");
    let third = cx
        .debug_bounds("toast-action-1-2")
        .expect("第三个按钮应显示");
    assert!(first.left() < second.left() && second.left() < third.left());

    cx.simulate_click(second.center(), gpui::Modifiers::default());
    assert_eq!(chosen.get(), Some(1));
    assert!(cx.debug_bounds("toast-1").is_none(), "操作后只关闭本条提示");
    assert!(cx.debug_bounds("toast-0").is_some(), "已有提示仍保留");
}
