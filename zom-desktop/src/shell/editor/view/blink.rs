//! 光标闪烁。
//!
//! 整个窗口共享一只闪烁时钟 —— 主编辑区、文件树新建条目、项目选择器查询框
//! 三处 [`super::EditorElement`] 共用同一相位。状态分两份：
//!
//! - [`CaretBlink`]：状态机（visible / epoch / 上一帧 cursor），由唯一的"驱动者"
//!   持有（目前是 shell 根视图）。
//! - [`CaretClock`]：GPUI Global，承载当前是否可见的纯读位 —— 由 [`drive`]
//!   在每帧 / 每次 tick 写入，由 [`super::element::EditorElement::paint`] 读。
//!
//! 嵌入点不再各自接收 `caret_visible` 参数：元素自己在 paint 时问 [`CaretClock`]。

use std::time::Duration;

use gpui::{App, Context, Global};

/// 光标明灭的半周期。530ms 是多数编辑器的常见取值。
pub(crate) const CARET_BLINK_INTERVAL: Duration = Duration::from_millis(530);

/// 光标闪烁状态机。
pub(crate) struct CaretBlink {
    visible: bool,
    /// 定时链代际：每次重置自增，旧链 tick 时代际不符即自行终止。
    epoch: usize,
    /// 上一帧观察到的光标字节位；用于检测"光标移动"从而重置闪烁。
    last_cursor: usize,
}

impl CaretBlink {
    pub(crate) fn new() -> Self {
        Self {
            visible: true,
            epoch: 0,
            // usize::MAX：与任何真实光标位都不同，保证首帧触发一次重置，借此启动定时链。
            last_cursor: usize::MAX,
        }
    }

    pub(crate) fn visible(&self) -> bool {
        self.visible
    }

    fn cursor_moved(&mut self, cursor_byte: usize) -> bool {
        let moved = cursor_byte != self.last_cursor;
        self.last_cursor = cursor_byte;
        moved
    }

    /// 一次定时翻转。代际不符说明已被 [`Self::restart`] 作废，返回 `false`
    /// 让调用方放弃这条旧定时链。
    fn tick(&mut self, epoch: usize) -> bool {
        if epoch != self.epoch {
            return false;
        }
        self.visible = !self.visible;
        true
    }

    /// 光标活动后重置：立即实心显示，并作废旧定时链。返回新代际。
    fn restart(&mut self) -> usize {
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

/// 每窗口共享的"当前是否绘制光标"读位。
///
/// 由 [`drive`] 在每帧 / 每次 tick 写入；[`super::EditorElement::paint`] 读取。
/// 默认 `visible = true`，避免首帧出现"半秒空窗"。
pub(crate) struct CaretClock {
    visible: bool,
}

impl Default for CaretClock {
    fn default() -> Self {
        Self { visible: true }
    }
}

impl Global for CaretClock {}

impl CaretClock {
    pub(crate) fn is_visible(cx: &App) -> bool {
        cx.try_global::<CaretClock>()
            .map(|c| c.visible)
            .unwrap_or(true)
    }

    fn write_visible(cx: &mut App, visible: bool) {
        cx.default_global::<CaretClock>().visible = visible;
    }
}

/// 每帧由驱动者调用一次：传入当前活动 cursor byte。
///
/// 内部职责：
/// 1. 检测光标是否相对上一帧移动；移动则重置 [`CaretBlink`] 并调度新一轮定时链。
/// 2. 把当前可见位写入 [`CaretClock`]。
///
/// `get_blink` 用于在异步 tick 闭包里从驱动者 view 上重新拿到 `&mut CaretBlink`，
/// 因为闭包不能持外部可变借用。
pub(crate) fn drive<V: 'static, F>(
    blink: &mut CaretBlink,
    cursor_byte: usize,
    cx: &mut Context<V>,
    get_blink: F,
) where
    F: Fn(&mut V) -> &mut CaretBlink + Clone + 'static,
{
    if blink.cursor_moved(cursor_byte) {
        let epoch = blink.restart();
        schedule_tick(epoch, cx, get_blink);
    }
    CaretClock::write_visible(cx, blink.visible());
}

fn schedule_tick<V: 'static, F>(epoch: usize, cx: &mut Context<V>, get_blink: F)
where
    F: Fn(&mut V) -> &mut CaretBlink + Clone + 'static,
{
    let next = get_blink.clone();
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(CARET_BLINK_INTERVAL).await;
        this.update(cx, |v, cx| {
            let blink = get_blink(v);
            if blink.tick(epoch) {
                let visible = blink.visible();
                CaretClock::write_visible(cx, visible);
                cx.notify();
                schedule_tick(epoch, cx, next);
            }
        })
        .ok();
    })
    .detach();
}
