//! 终端测试夹具：装配不启动 PTY 的 display-only 终端。
//!
//! 生产终端统一经 `TerminalBuilder::build` 启动真实 PTY；测试只验证渲染与交互行为时，
//! 直接在这里构造 `Terminal`，避免为测试在生产类型上保留构造入口。

use std::sync::Arc;

use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use async_channel::unbounded;
use gpui::{Context, Pixels};

use crate::{
    PtyResources, Terminal, TerminalBounds, TerminalBuilder, TerminalLifecycle, TerminalSettings,
    alacritty, configured_shell_name,
    pty_info::{ProcessIdGetter, PtyProcessInfo},
};

/// 构造一个不持有 PTY 资源的终端，供渲染与交互测试使用。
pub(crate) fn display_only_terminal(
    builder: &TerminalBuilder,
    cx: &mut Context<Terminal>,
) -> Terminal {
    let settings = TerminalSettings::load(cx, None);
    let bounds = TerminalBounds::default();
    let (events_tx, _) = unbounded();
    let term = alacritty::new_term(
        &alacritty::pty_term_config(settings.max_scroll_history_lines, settings.cursor_shape),
        &bounds,
        &events_tx,
        settings.alternate_scroll,
    );
    let initial_content = alacritty::make_content(&term.lock(), None);

    let pid_getter = ProcessIdGetter::new(-1, 0);

    Terminal {
        term,
        pty_resources: PtyResources::Released,
        events: Default::default(),
        events_rx: None,
        event_loop_task: None,
        last_content: initial_content,
        title: None,
        shell_name: configured_shell_name(settings.shell.as_deref()),
        scroll_px: Pixels::ZERO,
        process_info: Arc::new(PtyProcessInfo::new(pid_getter)),
        background_executor: cx.background_executor().clone(),
        lifecycle: TerminalLifecycle::Running,
        cwd: builder.cwd.clone(),
        font_size_override: None,
        mouse_gesture: None,
        selection_drag: None,
        selection_autoscroll_scheduled: false,
    }
}

/// 把字节直接写入模拟器网格，绕过 PTY。
pub(crate) fn write_output(terminal: &mut Terminal, bytes: &[u8], cx: &mut Context<Terminal>) {
    let mut processor = Processor::<StdSyncHandler>::new();
    processor.advance(&mut *terminal.term.lock(), bytes);
    cx.notify();
}
