//! 光标闪烁管理器。
//!
//! 参考 zed `crates/editor/src/blink_manager.rs`。
//! 通过递归异步定时器实现光标可见性的交替切换。
//! 闪烁循环仅在 `enable()` 后启动，`disable()` 后停止。

use std::time::Duration;

use gpui::Context;

/// 光标闪烁间隔。
const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);

pub struct BlinkManager {
    blink_interval: Duration,
    blink_epoch: usize,
    blinking_paused: bool,
    visible: bool,
    enabled: bool,
}

impl BlinkManager {
    pub fn new() -> Self {
        Self {
            blink_interval: CURSOR_BLINK_INTERVAL,
            blink_epoch: 0,
            blinking_paused: false,
            visible: true,
            enabled: false,
        }
    }

    fn next_blink_epoch(&mut self) -> usize {
        self.blink_epoch += 1;
        self.blink_epoch
    }

    /// 输入时暂停闪烁：立即显示光标，500ms 后恢复闪烁。
    pub fn pause_blinking(&mut self, cx: &mut Context<Self>) {
        self.show_cursor(cx);

        let epoch = self.next_blink_epoch();
        let interval = Duration::from_millis(500);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(interval).await;
            let _ = this.update(cx, |this, cx| this.resume_cursor_blinking(epoch, cx));
        })
        .detach();
    }

    fn resume_cursor_blinking(&mut self, epoch: usize, cx: &mut Context<Self>) {
        if epoch == self.blink_epoch {
            self.blinking_paused = false;
            self.blink_cursors(self.blink_epoch, cx);
        }
    }

    /// 递归闪烁：翻转 visible → 触发重新渲染 → 间隔后再次调用。
    fn blink_cursors(&mut self, epoch: usize, cx: &mut Context<Self>) {
        if epoch == self.blink_epoch && self.enabled && !self.blinking_paused {
            self.visible = !self.visible;
            cx.notify();

            let epoch = self.next_blink_epoch();
            let interval = self.blink_interval;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(interval).await;
                let _ = this.update(cx, |this, cx| this.blink_cursors(epoch, cx));
            })
            .detach();
        }
    }

    /// 确保光标可见。
    pub fn show_cursor(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            self.visible = true;
            cx.notify();
        }
    }

    /// 启用闪烁（编辑器获得焦点时调用）。
    pub fn enable(&mut self, cx: &mut Context<Self>) {
        if self.enabled {
            return;
        }
        self.enabled = true;
        // 先设为不可见，blink_cursors 会立刻翻转为 true，
        // 这样下一帧渲染时光标就是可见的。
        self.visible = false;
        self.blink_cursors(self.blink_epoch, cx);
    }

    /// 禁用闪烁（编辑器失去焦点时调用）。
    pub fn disable(&mut self, cx: &mut Context<Self>) {
        let was_visible = self.visible;
        self.visible = false;
        self.enabled = false;
        if was_visible {
            cx.notify();
        }
    }

    /// 光标当前应绘制。
    pub fn visible(&self) -> bool {
        self.visible
    }

    #[cfg(test)]
    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }
}
