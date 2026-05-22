//! 光标闪烁状态。
//!
//! 闪烁需要跨帧持久状态 + 定时器，故状态存在根视图（一个 GPUI Entity）上、
//! 由其自驱动定时链翻转。本模块只持「可见与否 / 定时链代际」这点纯状态，
//! 不碰 GPUI —— 定时器调度留给根视图。

use std::time::Duration;

/// 光标明灭的半周期。530ms 是多数编辑器的常见取值。
pub(crate) const CARET_BLINK_INTERVAL: Duration = Duration::from_millis(530);

/// 光标闪烁状态机。
pub(crate) struct CaretBlink {
    /// 当前是否显示光标。
    visible: bool,
    /// 定时链代际：每次重置自增，旧链 tick 时代际不符即自行终止。
    epoch: usize,
    /// 上一帧观察到的光标字节位；用于检测「光标移动」从而重置闪烁。
    last_cursor: usize,
}

impl CaretBlink {
    pub(crate) fn new() -> Self {
        Self {
            visible: true,
            epoch: 0,
            // usize::MAX：与任何真实光标位都不同，保证首帧触发一次重置 ——
            // 借此启动定时链。
            last_cursor: usize::MAX,
        }
    }

    pub(crate) fn visible(&self) -> bool {
        self.visible
    }

    /// 光标是否相对上一帧移动了；顺带记录新位置。
    pub(crate) fn cursor_moved(&mut self, cursor_byte: usize) -> bool {
        let moved = cursor_byte != self.last_cursor;
        self.last_cursor = cursor_byte;
        moved
    }

    /// 一次定时翻转。代际不符说明已被 [`Self::restart`] 作废，返回 `false`
    /// 让调用方放弃这条旧定时链。
    pub(crate) fn tick(&mut self, epoch: usize) -> bool {
        if epoch != self.epoch {
            return false;
        }
        self.visible = !self.visible;
        true
    }

    /// 光标活动后重置：立即实心显示，并作废旧定时链。返回新代际，调用方
    /// 据此调度新的定时链。
    pub(crate) fn restart(&mut self) -> usize {
        self.visible = true;
        self.epoch += 1;
        self.epoch
    }
}

impl Default for CaretBlink {
    fn default() -> Self {
        Self::new()
    }
}
