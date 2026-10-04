use std::time::Duration;

use gpui::{AppContext as _, TestAppContext};

use super::BlinkManager;

impl BlinkManager {
    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }
}

/// 禁用必须显式取消在途定时器，而不是等它在下次回调时自行失效。
#[gpui::test]
fn disable_cancels_the_in_flight_blink_timer(cx: &mut TestAppContext) {
    let manager = cx.new(|_| BlinkManager::new(true));
    manager.update(cx, |manager, cx| manager.enable(cx));
    assert!(
        cx.read_entity(&manager, |manager, _| manager.timer_task.is_some()),
        "启用后必须存在在途闪烁定时任务"
    );

    manager.update(cx, |manager, cx| manager.disable(cx));
    assert!(
        cx.read_entity(&manager, |manager, _| manager.timer_task.is_none()),
        "禁用必须显式取消在途定时任务"
    );

    // 取消后即使走过完整闪烁间隔也不得再翻转可见性。
    cx.executor().advance_clock(Duration::from_millis(2000));
    cx.run_until_parked();
    assert!(
        cx.read_entity(&manager, |manager, _| !manager.visible()),
        "取消后不得再继续闪烁"
    );
}

#[gpui::test]
fn disabled_blink_setting_keeps_focused_cursor_visible(cx: &mut TestAppContext) {
    let manager = cx.new(|_| BlinkManager::new(false));
    manager.update(cx, |manager, cx| manager.enable(cx));
    cx.read_entity(&manager, |manager, _| {
        assert!(manager.visible());
        assert!(manager.timer_task.is_none());
    });

    manager.update(cx, |manager, cx| manager.set_blink_enabled(true, cx));
    cx.read_entity(&manager, |manager, _| {
        assert!(manager.visible());
        assert!(manager.timer_task.is_some());
    });

    manager.update(cx, |manager, cx| manager.set_blink_enabled(false, cx));
    cx.read_entity(&manager, |manager, _| {
        assert!(manager.visible());
        assert!(manager.timer_task.is_none());
    });
}
