//! alacritty_terminal fork 的薄封装：类型别名、事件监听器、PTY 发送器、构造序列与操作函数。
//!
//! 依赖的 alacritty_terminal 是 Zed 维护的 fork（MIT/Apache-2.0），与上游差异仅在 tty 进程组管理。
//! 本模块参考 Zed 对上游的职责划分（其代码为 GPL-3.0，本文件独立实现）。

#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::{borrow::Cow, io, path::PathBuf, sync::Arc};

use alacritty_terminal::{
    event::{Event as AlacTermEvent, EventListener, Notify, WindowSize},
    event_loop::{EventLoop, Msg, Notifier},
    grid::{Dimensions, GridCell, Scroll as AlacScroll},
    index::{Column, Direction, Line, Point as AlacPoint},
    selection::{Selection as AlacSelection, SelectionType as AlacSelectionType},
    sync::FairMutex,
    term::{Config, Osc52, SEMANTIC_ESCAPE_CHARS, Term, TermMode, cell::Cell as AlacCell},
    tty,
    vte::ansi::{ClearMode, CursorShape, CursorStyle, Handler, NamedPrivateMode, PrivateMode},
};
use anyhow::Context as _;
use async_channel::Sender;

use crate::{
    Cell, Content, Cursor, IndexedCell, Modes, Point, PtyEvent, Scroll, Selection, SelectionRange,
    SelectionSide, SelectionType, TerminalBounds, pty_info::ProcessIdGetter,
};

pub(super) type AlacrittyTerm = Term<TerminalListener>;
pub(super) type AlacrittyTermLock = FairMutex<AlacrittyTerm>;
pub(super) type AlacrittyCell = AlacCell;

/// 把 alacritty 终端产生的事件转发到主线程的事件通道。
#[derive(Clone)]
pub(super) struct TerminalListener {
    events_tx: Sender<PtyEvent>,
}

impl EventListener for TerminalListener {
    fn send_event(&self, event: AlacTermEvent) {
        let event = match event {
            AlacTermEvent::MouseCursorDirty => return,
            AlacTermEvent::Title(title) => PtyEvent::Title(Some(title)),
            AlacTermEvent::ResetTitle => PtyEvent::Title(None),
            AlacTermEvent::ClipboardStore(_, text) => PtyEvent::ClipboardStore(text),
            AlacTermEvent::ClipboardLoad(_, formatter) => PtyEvent::ClipboardLoad(formatter),
            AlacTermEvent::ColorRequest(index, formatter) => {
                PtyEvent::ColorRequest(index, formatter)
            }
            AlacTermEvent::CursorBlinkingChange => return,
            AlacTermEvent::PtyWrite(bytes) => PtyEvent::PtyWrite(bytes.into_bytes()),
            AlacTermEvent::TextAreaSizeRequest(formatter) => {
                PtyEvent::TextAreaSizeRequest(formatter)
            }
            AlacTermEvent::Wakeup => PtyEvent::Wakeup,
            AlacTermEvent::Bell => PtyEvent::Bell,
            AlacTermEvent::Exit => PtyEvent::Exit,
            AlacTermEvent::ChildExit(status) => PtyEvent::ChildExit(status),
        };
        // 事件通道在 IO 线程发送；终端销毁后忽略发送失败。
        let _ = self.events_tx.try_send(event);
    }
}

/// 向 alacritty 事件循环线程发送消息的唯一句柄（写输入、调整尺寸、关闭）。
pub(super) enum PtySender {
    Live {
        notifier: Notifier,
    },
    #[cfg(test)]
    Inert,
}

impl PtySender {
    /// 把输入字节写入 PTY。
    pub(super) fn notify(&self, input: impl Into<Cow<'static, [u8]>>) {
        match self {
            Self::Live { notifier } => notifier.notify(input),
            #[cfg(test)]
            Self::Inert => {}
        }
    }

    /// 通知 PTY 调整窗口尺寸（触发 SIGWINCH 与 shell 的 resize 感知）。
    pub(super) fn resize(&self, bounds: &TerminalBounds) {
        #[cfg(not(test))]
        let Self::Live { notifier } = self;
        #[cfg(test)]
        let Some(notifier) = (match self {
            Self::Live { notifier } => Some(notifier),
            Self::Inert => None,
        }) else {
            return;
        };

        if let Err(error) = notifier
            .0
            .send(Msg::Resize(window_size_from_bounds(bounds)))
        {
            eprintln!("终端 PTY 调整尺寸失败：{error}");
        }
    }

    /// 优雅关闭事件循环线程。
    pub(super) fn shutdown(&self) {
        #[cfg(not(test))]
        let Self::Live { notifier } = self;
        #[cfg(test)]
        let Some(notifier) = (match self {
            Self::Live { notifier } => Some(notifier),
            Self::Inert => None,
        }) else {
            return;
        };

        if let Err(error) = notifier.0.send(Msg::Shutdown) {
            eprintln!("终端 PTY 关闭失败：{error}");
        }
    }

    #[cfg(test)]
    pub(super) fn inert() -> Self {
        Self::Inert
    }
}

pub(super) fn window_size_from_bounds(bounds: &TerminalBounds) -> WindowSize {
    WindowSize {
        num_lines: bounds.num_lines() as u16,
        num_cols: bounds.num_columns() as u16,
        cell_width: f32::from(bounds.cell_width()) as u16,
        cell_height: f32::from(bounds.line_height()) as u16,
    }
}

/// 构造 alacritty 配置：滚动回看上限、默认光标形状、语义选择分隔符与 OSC52 策略。
pub(super) fn pty_term_config(scrolling_history: usize, cursor_shape: CursorShape) -> Config {
    Config {
        scrolling_history,
        default_cursor_style: CursorStyle {
            shape: cursor_shape,
            blinking: false,
        },
        // 追加制表符作为语义选择的额外分隔符。
        semantic_escape_chars: format!("{SEMANTIC_ESCAPE_CHARS}─"),
        osc52: Osc52::OnlyCopy,
        ..Config::default()
    }
}

/// 组装 PTY 启动参数：shell 程序、工作目录、环境变量与信号掩码。
pub(super) fn pty_options(
    shell: Option<(String, Vec<String>)>,
    working_directory: Option<PathBuf>,
    env: std::collections::HashMap<String, String>,
) -> tty::Options {
    tty::Options {
        shell: shell.map(|(program, args)| tty::Shell::new(program, args)),
        working_directory,
        drain_on_exit: true,
        env,
        #[cfg(not(windows))]
        child_signal_mask: tty::SignalMask::current().ok(),
        #[cfg(windows)]
        escape_args: false,
    }
}

/// 打开 PTY 并启动 shell 子进程。
pub(super) fn open_pty(
    options: &tty::Options,
    bounds: &TerminalBounds,
    window_id: u64,
) -> io::Result<tty::Pty> {
    tty::new(options, window_size_from_bounds(bounds), window_id)
}

#[cfg(unix)]
pub(super) fn process_id_getter(pty: &tty::Pty) -> ProcessIdGetter {
    ProcessIdGetter::new(pty.file().as_raw_fd(), pty.child().id())
}

#[cfg(all(test, unix))]
pub(super) fn process_id_getter_for_test() -> ProcessIdGetter {
    ProcessIdGetter::new(-1, 0)
}

#[cfg(windows)]
pub(super) fn process_id_getter(pty: &tty::Pty) -> ProcessIdGetter {
    let fallback_pid = pty.child_watcher().pid().map(u32::from).unwrap_or_default();
    ProcessIdGetter::new(fallback_pid)
}

#[cfg(all(test, windows))]
pub(super) fn process_id_getter_for_test() -> ProcessIdGetter {
    ProcessIdGetter::new(0)
}

/// 创建终端模拟器实例（网格状态机），包裹在公平锁中以供 IO 线程与 UI 线程共享。
pub(super) fn new_term(
    config: &Config,
    bounds: &TerminalBounds,
    events_tx: &Sender<PtyEvent>,
    alternate_scroll: bool,
) -> Arc<AlacrittyTermLock> {
    let mut term = Term::new(
        config.clone(),
        bounds,
        TerminalListener {
            events_tx: events_tx.clone(),
        },
    );
    if !alternate_scroll {
        term.unset_private_mode(PrivateMode::Named(NamedPrivateMode::AlternateScroll));
    }
    Arc::new(FairMutex::new(term))
}

/// 启动 PTY IO 线程并返回发送句柄。
pub(super) fn spawn_event_loop(
    term: Arc<AlacrittyTermLock>,
    events_tx: &Sender<PtyEvent>,
    pty: tty::Pty,
    drain_on_exit: bool,
) -> anyhow::Result<PtySender> {
    let event_loop = EventLoop::new(
        term,
        TerminalListener {
            events_tx: events_tx.clone(),
        },
        pty,
        drain_on_exit,
        false,
    )
    .context("创建终端事件循环失败")?;
    let pty_tx = event_loop.channel();
    // 线程随 Msg::Shutdown 优雅退出，句柄无需保留。
    let _io_thread = event_loop.spawn();
    Ok(PtySender::Live {
        notifier: Notifier(pty_tx),
    })
}

pub(super) fn resize(term: &mut AlacrittyTerm, bounds: &TerminalBounds) {
    term.resize(*bounds);
}

#[cfg(test)]
pub(super) fn write_output(term: &mut AlacrittyTerm, bytes: &[u8]) {
    let mut processor = alacritty_terminal::vte::ansi::Processor::<
        alacritty_terminal::vte::ansi::StdSyncHandler,
    >::new();
    processor.advance(term, bytes);
}

/// 更新既有选择到新位置；没有进行中的选择时返回 false。
pub(super) fn update_selection(
    term: &mut AlacrittyTerm,
    point: Point,
    side: SelectionSide,
) -> bool {
    let Some(mut selection) = term.selection.take() else {
        return false;
    };
    selection.update(point.to_alacritty(), side.to_alacritty());
    term.selection = Some(selection);
    true
}

/// 清除滚动回看，保留光标行（prompt）并移到屏幕顶部。
pub(super) fn clear(term: &mut AlacrittyTerm) {
    term.clear_screen(ClearMode::Saved);
    let cursor = term.grid().cursor.point;
    term.grid_mut().reset_region(..cursor.line);
    let line = term.grid()[cursor.line][..Column(term.grid().columns())].to_vec();
    for (index, cell) in line.into_iter().enumerate() {
        term.grid_mut()[Line(0)][Column(index)] = cell;
    }
    term.grid_mut().cursor.point = AlacPoint::new(Line(0), cursor.column);
    let new_cursor = term.grid().cursor.point;
    if (new_cursor.line.0 as usize) < term.screen_lines() - 1 {
        term.grid_mut().reset_region((new_cursor.line + 1)..);
    }
}

/// 生成渲染快照：一次性读取网格、光标、模式与选择状态。
///
/// 坐标约定：网格单元格与选择均使用绝对坐标（滚动回看顶行为负行号）；
/// 光标同样是绝对坐标，这里统一换算成视口行（0 = 视口顶），渲染层直接定位。
/// 泛型化以支持测试用 `Term<VoidListener>`（mock_term）。
pub(super) fn make_content<T: EventListener>(
    term: &Term<T>,
    last_content: Option<&Content>,
) -> Content {
    let content = term.renderable_content();
    let display_offset = content.display_offset;
    let cells = content
        .display_iter
        .map(|indexed| {
            let point = Point {
                line: indexed.point.line.0,
                column: indexed.point.column.0,
            };
            IndexedCell {
                point,
                cell: Cell::new(indexed.cell.clone()),
            }
        })
        .collect();

    let selection_text = content.selection.and_then(|_| term.selection_to_string());
    let grid = term.grid();
    // 光标点是网格绝对行（滚动时不变，跟随内容）；视口行 = 绝对行 + 回看偏移。
    let cursor = Cursor {
        shape: content.cursor.shape,
        point: Point {
            line: content.cursor.point.line.0 + display_offset as i32,
            column: content.cursor.point.column.0,
        },
    };
    let cursor_cell = Cell::new(grid[content.cursor.point].clone());
    let bottom_row_occupied = grid
        .display_iter()
        .last()
        .map(|indexed| !indexed.cell.is_empty())
        .unwrap_or(false);

    Content {
        cells,
        mode: Modes::from_alacritty(content.mode),
        total_lines: grid.total_lines(),
        display_offset,
        columns: grid.columns(),
        screen_lines: grid.screen_lines(),
        selection_text,
        selection: content.selection.map(|range| SelectionRange {
            start: Point {
                line: range.start.line.0,
                column: range.start.column.0,
            },
            end: Point {
                line: range.end.line.0,
                column: range.end.column.0,
            },
            is_block: range.is_block,
        }),
        cursor,
        cursor_cell,
        terminal_bounds: last_content.map_or_else(TerminalBounds::default, |c| c.terminal_bounds),
        scrolled_to_top: display_offset == grid.history_size(),
        scrolled_to_bottom: display_offset == 0,
        bottom_row_occupied,
    }
}

// ─── 坐标与类型映射 ────────────────────────────────────────────────

impl Scroll {
    pub(super) fn to_alacritty(self) -> AlacScroll {
        match self {
            Scroll::Delta(lines) => AlacScroll::Delta(lines),
            Scroll::Bottom => AlacScroll::Bottom,
        }
    }
}

impl Point {
    pub(super) fn to_alacritty(self) -> AlacPoint {
        AlacPoint::new(self.line.into(), self.column.into())
    }
}

impl SelectionSide {
    pub(super) fn to_alacritty(self) -> Direction {
        match self {
            SelectionSide::Left => Direction::Left,
            SelectionSide::Right => Direction::Right,
        }
    }
}

impl SelectionType {
    pub(super) fn to_alacritty(self) -> AlacSelectionType {
        match self {
            SelectionType::Simple => AlacSelectionType::Simple,
            SelectionType::Semantic => AlacSelectionType::Semantic,
            SelectionType::Lines => AlacSelectionType::Lines,
        }
    }
}

impl Selection {
    pub(super) fn to_alacritty(&self) -> AlacSelection {
        let mut selection = AlacSelection::new(
            self.ty.to_alacritty(),
            self.start.point.to_alacritty(),
            self.start.side.to_alacritty(),
        );
        selection.update(self.end.point.to_alacritty(), self.end.side.to_alacritty());
        selection
    }
}

impl Modes {
    pub(super) fn from_alacritty(mode: TermMode) -> Modes {
        let mut result = Modes::empty();
        if mode.contains(TermMode::SHOW_CURSOR) {
            result |= Modes::SHOW_CURSOR;
        }
        if mode.contains(TermMode::APP_CURSOR) {
            result |= Modes::APP_CURSOR;
        }
        if mode.contains(TermMode::APP_KEYPAD) {
            result |= Modes::APP_KEYPAD;
        }
        if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            result |= Modes::MOUSE_REPORT_CLICK;
        }
        if mode.contains(TermMode::MOUSE_MOTION) {
            result |= Modes::MOUSE_MOTION;
        }
        if mode.contains(TermMode::MOUSE_DRAG) {
            result |= Modes::MOUSE_DRAG;
        }
        if mode.contains(TermMode::SGR_MOUSE) {
            result |= Modes::SGR_MOUSE;
        }
        if mode.contains(TermMode::UTF8_MOUSE) {
            result |= Modes::UTF8_MOUSE;
        }
        if mode.contains(TermMode::LINE_WRAP) {
            result |= Modes::LINE_WRAP;
        }
        if mode.contains(TermMode::INSERT) {
            result |= Modes::INSERT;
        }
        if mode.contains(TermMode::ALT_SCREEN) {
            result |= Modes::ALT_SCREEN;
        }
        if mode.contains(TermMode::ALTERNATE_SCROLL) {
            result |= Modes::ALTERNATE_SCROLL;
        }
        if mode.contains(TermMode::BRACKETED_PASTE) {
            result |= Modes::BRACKETED_PASTE;
        }
        if mode.contains(TermMode::FOCUS_IN_OUT) {
            result |= Modes::FOCUS_IN_OUT;
        }
        result
    }
}

impl Dimensions for TerminalBounds {
    fn total_lines(&self) -> usize {
        self.num_lines()
    }

    fn screen_lines(&self) -> usize {
        self.num_lines()
    }

    fn columns(&self) -> usize {
        self.num_columns()
    }
}
