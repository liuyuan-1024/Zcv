//! zcv-terminal：嵌入式终端面板。
//!
//! 模拟器核心使用 Zed 维护的 alacritty_terminal fork（MIT/Apache-2.0），本 crate 提供薄封装（`alacritty` 模块）、终端状态机（`Terminal`）与视图/面板（`view` / `panel` 模块）。
//! 事件流分层：alacritty IO 线程 → 事件通道 → 主线程批处理队列 → 每帧渲染快照。

mod alacritty;
mod element;
mod mappings;
mod palette;
mod panel;
mod pty_info;
mod view;

#[cfg(test)]
mod test;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::ExitStatus,
    sync::Arc,
    time::Duration,
};

use alacritty_terminal::{
    term::cell::Flags,
    vte::ansi::{Color, CursorShape, Rgb},
};
use anyhow::{Context as _, Result};
use async_channel::{Receiver, unbounded};
use gpui::{App, BackgroundExecutor, Context, EventEmitter, Pixels, Size, Task, Window};
pub use panel::TerminalPanel;
pub(crate) use view::TerminalView;

use crate::{
    alacritty::{AlacrittyTermLock, PtySender},
    pty_info::PtyProcessInfo,
};

/// 关闭终端后给 shell 与前台任务的优雅退出宽限期，超时后升级为 SIGKILL。
/// 必须低于 gpui 的退出超时，保证应用退出时升级也能完成。
const PROCESS_KILL_GRACE_PERIOD: Duration = Duration::from_millis(100);
/// 调试用的默认终端尺寸（创建后由视图第一帧真实尺寸覆盖）。
const DEBUG_TERMINAL_WIDTH: f32 = 500.;
const DEBUG_TERMINAL_HEIGHT: f32 = 30.;
const DEBUG_CELL_WIDTH: f32 = 5.;
const DEBUG_LINE_HEIGHT: f32 = 5.;
/// 创建时未确定窗口 id 时的占位值（unix 上无实际用途）。
const DUMMY_WINDOW_ID: u64 = 0;

// ─── 尺寸与坐标 ───────────────────────────────────────────────────

/// 像素边界：由视图每帧计算，携带单元格宽高。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TerminalBounds {
    size: Size<Pixels>,
    cell_width: Pixels,
    cell_height: Pixels,
}

impl TerminalBounds {
    pub fn new(cell_width: Pixels, cell_height: Pixels, size: Size<Pixels>) -> Self {
        TerminalBounds {
            size,
            cell_width,
            cell_height,
        }
    }

    /// 可见行数；容忍浮点精度，避免少算一行。
    pub fn num_lines(&self) -> usize {
        let raw = f32::from(self.size.height) / f32::from(self.cell_height);
        raw.next_up().floor().max(1.) as usize
    }

    /// 可见列数；同样容忍浮点精度。
    pub fn num_columns(&self) -> usize {
        let raw = f32::from(self.size.width) / f32::from(self.cell_width);
        raw.next_up().floor().max(2.) as usize
    }

    pub fn cell_width(&self) -> Pixels {
        self.cell_width
    }

    pub fn line_height(&self) -> Pixels {
        self.cell_height
    }
}

impl Default for TerminalBounds {
    fn default() -> Self {
        TerminalBounds::new(
            Pixels::from(DEBUG_CELL_WIDTH),
            Pixels::from(DEBUG_LINE_HEIGHT),
            Size {
                width: Pixels::from(DEBUG_TERMINAL_WIDTH),
                height: Pixels::from(DEBUG_TERMINAL_HEIGHT),
            },
        )
    }
}

/// 网格坐标，行号为绝对坐标：滚动回看顶行为负行号。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct Point {
    pub line: i32,
    pub column: usize,
}

// ─── 渲染快照 ─────────────────────────────────────────────────────

/// 一次渲染所需的全部终端状态快照；渲染层只读快照，不触碰终端锁。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Content {
    /// 全部显示行（含滚动回看）的单元格，绝对坐标。
    pub cells: Vec<IndexedCell>,
    pub mode: Modes,
    pub total_lines: usize,
    pub display_offset: usize,
    pub columns: usize,
    pub screen_lines: usize,
    pub selection_text: Option<String>,
    pub selection: Option<SelectionRange>,
    pub cursor: Cursor,
    /// 光标格（含宽字符标志：光标块宽度按网格判定，不重复查 unicode 宽度表）。
    pub cursor_cell: Cell,
    pub terminal_bounds: TerminalBounds,
    pub scrolled_to_top: bool,
    pub scrolled_to_bottom: bool,
    /// 底部行是否有非空内容（底部锚定用）。
    pub bottom_row_occupied: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct IndexedCell {
    pub point: Point,
    pub cell: Cell,
}

/// 单元格的薄包装，公开样式查询接口；内部直接持有关联的 alacritty 单元格。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Cell {
    cell: alacritty::AlacrittyCell,
}

impl Cell {
    pub(crate) fn new(cell: alacritty::AlacrittyCell) -> Self {
        Cell { cell }
    }

    pub fn character(&self) -> char {
        self.cell.c
    }

    pub fn foreground(&self) -> Color {
        self.cell.fg
    }

    pub fn background(&self) -> Color {
        self.cell.bg
    }

    pub fn is_inverse(&self) -> bool {
        self.cell.flags.contains(Flags::INVERSE)
    }

    pub fn is_wide_char_spacer(&self) -> bool {
        self.cell.flags.contains(Flags::WIDE_CHAR_SPACER)
            || self.cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
    }

    pub fn is_wide_char(&self) -> bool {
        self.cell.flags.contains(Flags::WIDE_CHAR)
    }

    pub fn is_dim(&self) -> bool {
        self.cell.flags.contains(Flags::DIM)
    }

    pub fn is_bold(&self) -> bool {
        self.cell.flags.contains(Flags::BOLD)
    }

    pub fn is_italic(&self) -> bool {
        self.cell.flags.contains(Flags::ITALIC)
    }

    pub fn has_underline(&self) -> bool {
        self.cell.flags.contains(Flags::ALL_UNDERLINES)
    }

    pub fn has_strikeout(&self) -> bool {
        self.cell.flags.contains(Flags::STRIKEOUT)
    }

    pub fn zerowidth(&self) -> Option<&[char]> {
        self.cell.zerowidth()
    }
}

/// 光标：形状与网格坐标（绝对坐标）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub shape: CursorShape,
    pub point: Point,
}

/// 终端模式子集（渲染与输入映射需要的位）；完整模式保留在封装层。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Modes(u32);

impl Modes {
    pub const SHOW_CURSOR: Modes = Modes(1 << 0);
    pub const APP_CURSOR: Modes = Modes(1 << 1);
    pub const APP_KEYPAD: Modes = Modes(1 << 2);
    pub const MOUSE_REPORT_CLICK: Modes = Modes(1 << 3);
    pub const MOUSE_MOTION: Modes = Modes(1 << 4);
    pub const MOUSE_DRAG: Modes = Modes(1 << 5);
    pub const SGR_MOUSE: Modes = Modes(1 << 6);
    pub const UTF8_MOUSE: Modes = Modes(1 << 7);
    pub const LINE_WRAP: Modes = Modes(1 << 8);
    pub const INSERT: Modes = Modes(1 << 9);
    pub const ALT_SCREEN: Modes = Modes(1 << 10);
    pub const ALTERNATE_SCROLL: Modes = Modes(1 << 11);
    pub const BRACKETED_PASTE: Modes = Modes(1 << 12);
    pub const FOCUS_IN_OUT: Modes = Modes(1 << 13);
    pub const MOUSE_MODE: Modes =
        Modes(Self::MOUSE_REPORT_CLICK.0 | Self::MOUSE_MOTION.0 | Self::MOUSE_DRAG.0);
    pub const NONE: Modes = Modes(0);

    pub fn empty() -> Modes {
        Modes::NONE
    }

    pub fn contains(&self, other: Modes) -> bool {
        self.0 & other.0 == other.0
    }

    /// 与任一标志有交集（MOUSE_MODE 等合集判断用）。
    pub fn intersects(&self, other: Modes) -> bool {
        self.0 & other.0 != 0
    }
}

impl std::ops::BitOr for Modes {
    type Output = Modes;

    fn bitor(self, rhs: Modes) -> Modes {
        Modes(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Modes {
    fn bitor_assign(&mut self, rhs: Modes) {
        self.0 |= rhs.0;
    }
}

// ─── 选择 ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionType {
    Simple,
    Semantic,
    Lines,
}

/// 选择锚点所在格子的侧边（决定半格命中与扩展方向）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectionSide {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SelectionPoint {
    pub point: Point,
    pub side: SelectionSide,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Selection {
    pub ty: SelectionType,
    pub start: SelectionPoint,
    pub end: SelectionPoint,
}

/// 网格坐标下的选择范围（绝对坐标，start 在 end 之上）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SelectionRange {
    pub start: Point,
    pub end: Point,
    pub is_block: bool,
}

impl SelectionRange {
    pub fn contains(&self, point: Point) -> bool {
        point >= self.start && point <= self.end
    }
}

// ─── 滚动 ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scroll {
    Delta(i32),
    Bottom,
}

// ─── 事件 ─────────────────────────────────────────────────────────

/// PTY/终端层事件：由封装层监听器产出，经事件通道交给主线程。
pub(crate) enum PtyEvent {
    Title(Option<String>),
    ClipboardStore(String),
    ClipboardLoad(Arc<dyn Fn(&str) -> String + Sync + Send>),
    ColorRequest(usize, Arc<dyn Fn(Rgb) -> String + Sync + Send>),
    PtyWrite(Vec<u8>),
    TextAreaSizeRequest(Arc<dyn Fn(alacritty_terminal::event::WindowSize) -> String + Sync + Send>),
    Wakeup,
    Bell,
    Exit,
    ChildExit(ExitStatus),
}

/// 主线程待处理事件队列。
pub(crate) enum InternalEvent {
    Bell,
    Wakeup,
    Title(Option<String>),
    ClipboardStore(String),
    ClipboardLoad(Arc<dyn Fn(&str) -> String + Sync + Send>),
    ColorRequest(usize, Arc<dyn Fn(Rgb) -> String + Sync + Send>),
    PtyWrite(Vec<u8>),
    TextAreaSizeRequest(Arc<dyn Fn(alacritty_terminal::event::WindowSize) -> String + Sync + Send>),
    Resize(TerminalBounds),
    Scroll(Scroll),
    SetSelection(Option<Selection>),
    ChildExit(ExitStatus),
    Exit,
}

/// 终端向视图层上抛的事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    TitleChanged(Option<String>),
    Bell,
    Wakeup,
    SelectionsChanged,
}

impl EventEmitter<Event> for Terminal {}

// ─── 终端设置 ─────────────────────────────────────────────────────

/// 终端行为设置快照，从 zcv-settings 的用户配置读取。
#[derive(Clone, Debug)]
pub(crate) struct TerminalSettings {
    /// 终端字号：显式配置优先，缺省跟随内容字号。
    pub font_size: f32,
    pub line_height: f32,
    pub max_scroll_history_lines: usize,
    pub cursor_shape: CursorShape,
    pub alternate_scroll: bool,
    pub option_as_meta: bool,
    pub shell: Option<String>,
}

impl TerminalSettings {
    pub fn from_user_settings(settings: &zcv_settings::UserSettings) -> Self {
        TerminalSettings {
            font_size: settings.terminal_font_size.unwrap_or(settings.font_size),
            line_height: settings
                .terminal_line_height
                .unwrap_or(settings.line_height),
            max_scroll_history_lines: settings.terminal_max_scroll_history_lines,
            cursor_shape: match settings.terminal_cursor_shape.as_str() {
                "underline" => CursorShape::Underline,
                "bar" => CursorShape::Beam,
                "hollow" => CursorShape::HollowBlock,
                _ => CursorShape::Block,
            },
            alternate_scroll: settings.terminal_alternate_scroll,
            option_as_meta: settings.terminal_option_as_meta,
            shell: settings.terminal_shell.clone(),
        }
    }

    pub fn load(cx: &App) -> Self {
        zcv_settings::SettingsStore::try_get(cx)
            .map(|settings| Self::from_user_settings(&settings))
            .unwrap_or_else(|| Self::from_user_settings(&zcv_settings::UserSettings::default()))
    }
}

// ─── 终端构造参数 ────────────────────────────────────────────────

/// 终端构造参数：只承载工作目录，其余设置读取用户配置。
pub(crate) struct TerminalBuilder {
    cwd: Option<PathBuf>,
}

impl Default for TerminalBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalBuilder {
    pub fn new() -> Self {
        TerminalBuilder { cwd: None }
    }

    pub fn set_cwd(mut self, cwd: Option<PathBuf>) -> Self {
        self.cwd = cwd;
        self
    }

    pub fn build(&self, cx: &mut Context<Terminal>) -> Result<Terminal> {
        Terminal::new(self, cx)
    }
}

// ─── 终端 ─────────────────────────────────────────────────────────

/// 终端状态机：持有 alacritty 模拟器、PTY 发送句柄与主线程事件队列。
pub(crate) struct Terminal {
    term: Arc<AlacrittyTermLock>,
    pty_tx: PtySender,
    events: std::collections::VecDeque<InternalEvent>,
    events_rx: Option<Receiver<PtyEvent>>,
    event_loop_task: Option<Task<()>>,
    last_content: Option<Content>,
    title: Option<String>,
    shell_name: String,
    scroll_px: Pixels,
    pty_pid: Option<u32>,
    process_info: Arc<PtyProcessInfo>,
    background_executor: BackgroundExecutor,
    /// 当前工作目录（持久化恢复终端会话用）。
    cwd: Option<PathBuf>,
}

impl Terminal {
    pub fn new(builder: &TerminalBuilder, cx: &mut Context<Self>) -> Result<Terminal> {
        let settings = TerminalSettings::load(cx);
        let bounds = TerminalBounds::default();
        let shell_name = configured_shell_name(settings.shell.as_deref());

        // 注入终端环境变量，保证 shell 以终端语义启动。
        let env = HashMap::from([
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
        ]);

        // 解析 shell 程序：用户显式配置时原样启动。
        // 无配置时不传 shell，alacritty 在 macOS 上会经 `/usr/bin/login` 启动登录 shell（打印 "Last login"、argv[0] 前缀 `-` 触发登录模式，读取 /etc/zprofile 的path_helper 重建 PATH），nvm/Homebrew 安装的命令才可用；
        let shell = settings
            .shell
            .as_deref()
            .map(|program| (program.to_string(), Vec::new()));

        let config =
            alacritty::pty_term_config(settings.max_scroll_history_lines, settings.cursor_shape);
        let (events_tx, events_rx) = unbounded();
        let term = alacritty::new_term(&config, &bounds, &events_tx, settings.alternate_scroll);
        let pty = alacritty::open_pty(
            &alacritty::pty_options(shell, builder.cwd.clone(), env),
            &bounds,
            DUMMY_WINDOW_ID,
        )
        .context("启动终端失败：无法创建 PTY")?;
        let process_id_getter = alacritty::process_id_getter(&pty);
        let pty_pid = process_id_getter.fallback_pid().as_u32();
        let process_info = Arc::new(PtyProcessInfo::new(process_id_getter));
        let pty_tx = alacritty::spawn_event_loop(term.clone(), &events_tx, pty, true)?;
        let background_executor = cx.background_executor().clone();

        let mut terminal = Terminal {
            term,
            pty_tx,
            events: Default::default(),
            events_rx: Some(events_rx),
            event_loop_task: None,
            last_content: None,
            title: None,
            shell_name,
            scroll_px: Pixels::ZERO,
            pty_pid: Some(pty_pid),
            process_info,
            background_executor,
            cwd: builder.cwd.clone(),
        };
        terminal.spawn_event_loop(cx);

        // 设置变更不热更新到已运行终端，重启终端后生效（gpui 0.2.2 的全局订阅需要窗口句柄，
        // 终端创建时未必有窗口；MVP 从简）。

        Ok(terminal)
    }

    /// 启动主线程事件泵：收集一批事件（≤100）合并上抛，避免逐条唤醒渲染。
    fn spawn_event_loop(&mut self, cx: &mut Context<Self>) {
        let rx = self.events_rx.take().expect("事件通道只消费一次");
        let task = cx.spawn(async move |this, cx| {
            let mut batch = Vec::new();
            loop {
                batch.clear();
                match rx.recv().await {
                    Ok(event) => batch.push(event),
                    // 通道关闭：终端已销毁。
                    Err(_) => return,
                }
                // 非阻塞收敛同批事件。
                while batch.len() < 100 {
                    match rx.try_recv() {
                        Ok(event) => batch.push(event),
                        Err(_) => break,
                    }
                }
                if this
                    .update(cx, |terminal, cx| {
                        terminal.push_pty_events(std::mem::take(&mut batch), cx)
                    })
                    .is_err()
                {
                    return;
                }
            }
        });
        self.event_loop_task = Some(task);
    }

    fn push_pty_events(&mut self, events: Vec<PtyEvent>, cx: &mut Context<Self>) {
        for event in events {
            match event {
                PtyEvent::Title(title) => self.events.push_back(InternalEvent::Title(title)),
                PtyEvent::ClipboardStore(text) => {
                    self.events.push_back(InternalEvent::ClipboardStore(text))
                }
                PtyEvent::ClipboardLoad(formatter) => self
                    .events
                    .push_back(InternalEvent::ClipboardLoad(formatter)),
                PtyEvent::ColorRequest(index, formatter) => self
                    .events
                    .push_back(InternalEvent::ColorRequest(index, formatter)),
                PtyEvent::PtyWrite(bytes) => self.events.push_back(InternalEvent::PtyWrite(bytes)),
                PtyEvent::TextAreaSizeRequest(formatter) => self
                    .events
                    .push_back(InternalEvent::TextAreaSizeRequest(formatter)),
                PtyEvent::Wakeup => self.events.push_back(InternalEvent::Wakeup),
                PtyEvent::Bell => self.events.push_back(InternalEvent::Bell),
                PtyEvent::Exit => self.events.push_back(InternalEvent::Exit),
                PtyEvent::ChildExit(status) => {
                    self.events.push_back(InternalEvent::ChildExit(status))
                }
            }
        }
        cx.notify();
    }

    /// 视图每帧调用：通知尺寸变化（行列或格宽变化才入队，合并连续 resize）。
    pub fn set_size(&mut self, bounds: TerminalBounds, cx: &mut Context<Self>) {
        let changed = self
            .last_content
            .as_ref()
            .map(|content| content.terminal_bounds != bounds)
            .unwrap_or(true);
        if changed {
            if let Some(event) = self
                .events
                .iter_mut()
                .find(|event| matches!(event, InternalEvent::Resize(_)))
            {
                *event = InternalEvent::Resize(bounds);
            } else {
                self.events.push_back(InternalEvent::Resize(bounds));
            }
            cx.notify();
        }
    }

    /// 视图每帧调用：排空事件队列并刷新渲染快照。
    pub fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.process_terminal_events(window, cx);
        self.last_content = Some(alacritty::make_content(
            &self.term.lock(),
            self.last_content.as_ref(),
        ));
    }

    fn process_terminal_events(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for event in self.events.drain(..) {
            match event {
                InternalEvent::Resize(bounds) => {
                    // 先通知 PTY（触发 SIGWINCH），再调整网格。
                    self.pty_tx.resize(&bounds);
                    alacritty::resize(&mut self.term.lock(), &bounds);
                }
                InternalEvent::PtyWrite(bytes) => self.pty_tx.notify(bytes),
                InternalEvent::Scroll(scroll) => {
                    self.term.lock().scroll_display(scroll.to_alacritty());
                }
                InternalEvent::SetSelection(selection) => {
                    self.term.lock().selection = selection.map(|s| s.to_alacritty());
                    cx.emit(Event::SelectionsChanged);
                }
                InternalEvent::Title(title) => {
                    self.title = title.clone();
                    cx.emit(Event::TitleChanged(title));
                }
                InternalEvent::ClipboardStore(text) => {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                }
                InternalEvent::ClipboardLoad(formatter) => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.pty_tx.notify(formatter(&text).into_bytes());
                    }
                }
                InternalEvent::ColorRequest(index, formatter) => {
                    let color = self.term.lock().colors()[index].unwrap_or_else(|| {
                        to_vte_rgb(palette::get_color_at_index(index, window, cx))
                    });
                    self.pty_tx.notify(formatter(color).into_bytes());
                }
                InternalEvent::TextAreaSizeRequest(formatter) => {
                    let size = alacritty::window_size_from_bounds(
                        &self
                            .last_content
                            .as_ref()
                            .map_or_else(TerminalBounds::default, |content| {
                                content.terminal_bounds
                            }),
                    );
                    self.pty_tx.notify(formatter(size).into_bytes());
                }
                InternalEvent::Bell => {
                    cx.emit(Event::Bell);
                }
                InternalEvent::Wakeup => {
                    cx.emit(Event::Wakeup);
                    self.process_info.clone().refresh(cx);
                }
                InternalEvent::ChildExit(status) => {
                    eprintln!("终端子进程退出：{status}");
                }
                InternalEvent::Exit => {
                    eprintln!("终端退出");
                }
            }
        }
    }

    // ── 输入 ──

    /// 把输入字节写入 PTY（不附带 UI 状态变更）。
    pub fn write_to_pty(&self, bytes: Vec<u8>) {
        self.pty_tx.notify(bytes);
    }

    /// 用户输入：先滚到底部并清除选择，再写入 PTY。
    pub fn write_input(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        self.events.push_back(InternalEvent::Scroll(Scroll::Bottom));
        self.events.push_back(InternalEvent::SetSelection(None));
        self.pty_tx.notify(bytes);
        cx.notify();
    }

    // ── 滚动 ──

    pub fn scroll_lines(&mut self, lines: i32, cx: &mut Context<Self>) {
        self.events
            .push_back(InternalEvent::Scroll(Scroll::Delta(lines)));
        cx.notify();
    }

    /// 像素滚动：触控板手势按 touch phase 处理——Started 重置累积、Moved 累积并滚动整行、Ended/Cancelled 忽略。
    /// 慢滚时每次增量小，累积跨事件进行，保证手感连续。
    pub fn scroll_px(
        &mut self,
        phase: gpui::TouchPhase,
        delta: Pixels,
        line_height: Pixels,
        cx: &mut Context<Self>,
    ) {
        match phase {
            gpui::TouchPhase::Started => {
                self.scroll_px = Pixels::ZERO;
            }
            gpui::TouchPhase::Moved => {
                self.scroll_px += delta;
                let lines = (f32::from(self.scroll_px) / f32::from(line_height)) as i32;
                if lines != 0 {
                    // 保留余量：不足一行的像素跨事件继续累积。
                    self.scroll_px =
                        Pixels::from(f32::from(self.scroll_px) % f32::from(line_height));
                    self.scroll_lines(lines, cx);
                }
            }
            gpui::TouchPhase::Ended => {}
        }
    }

    // ── 选择 ──

    pub fn select_range(
        &mut self,
        ty: SelectionType,
        start: SelectionPoint,
        end: Option<SelectionPoint>,
        cx: &mut Context<Self>,
    ) {
        let selection = Selection {
            ty,
            start,
            end: end.unwrap_or(start),
        };
        self.events
            .push_back(InternalEvent::SetSelection(Some(selection)));
        cx.notify();
    }

    pub fn update_selection(&mut self, point: SelectionPoint, cx: &mut Context<Self>) -> bool {
        let updated = alacritty::update_selection(&mut self.term.lock(), point.point, point.side);
        if updated {
            cx.emit(Event::SelectionsChanged);
            cx.notify();
        }
        updated
    }

    /// 当前选择文本（来自最新渲染快照）。
    pub fn selection_text(&self) -> Option<String> {
        self.last_content.as_ref()?.selection_text.clone()
    }

    // ── 动作 ──

    /// 把选择复制到系统剪贴板。
    pub fn copy(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selection_text() {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        }
    }

    /// 把剪贴板内容粘贴进终端（bracketed paste 时加包裹）。
    pub fn paste(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let bytes = if self
                .last_content
                .as_ref()
                .map(|content| content.mode.contains(Modes::BRACKETED_PASTE))
                .unwrap_or(false)
            {
                format!("\x1b[200~{text}\x1b[201~").into_bytes()
            } else {
                text.into_bytes()
            };
            self.write_input(bytes, cx);
        }
    }

    /// 清空屏幕与滚动回看。
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        alacritty::clear(&mut self.term.lock());
        cx.notify();
    }

    pub fn tab_title(&self) -> String {
        let directory_name = self
            .cwd
            .as_deref()
            .and_then(path_name)
            .unwrap_or_else(|| "终端".to_owned());
        format!("{directory_name} — {}", self.shell_name)
    }

    /// 当前工作目录。
    pub fn working_directory(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    pub fn last_content(&self) -> Option<&Content> {
        self.last_content.as_ref()
    }

    /// 更新聚焦状态，向终端报告 focus in/out（focus reporting 模式）。
    pub fn focus_terminal(&mut self, focused: bool, cx: &mut Context<Self>) {
        let term = &mut *self.term.lock();
        if term.is_focused == focused {
            return;
        }
        term.is_focused = focused;
        if self
            .last_content
            .as_ref()
            .map(|content| content.mode.contains(Modes::FOCUS_IN_OUT))
            .unwrap_or(false)
        {
            self.pty_tx.notify(if focused {
                b"\x1b[I".as_slice()
            } else {
                b"\x1b[O".as_slice()
            });
        }
        cx.notify();
    }

    /// 优雅终止 shell 进程组并关闭 PTY。
    fn kill_current_process(&mut self) {
        let Some(pid) = self.pty_pid.take() else {
            return;
        };
        #[cfg(unix)]
        {
            let pid = pid as i32;
            let executor = self.background_executor.clone();
            // 先发 SIGTERM 到进程组，宽限期后升级 SIGKILL。
            unsafe {
                libc::killpg(pid, libc::SIGTERM);
            }
            let timer = executor.clone();
            executor
                .spawn(async move {
                    timer.timer(PROCESS_KILL_GRACE_PERIOD).await;
                    unsafe {
                        libc::killpg(pid, libc::SIGKILL);
                    }
                })
                .detach();
        }
    }
}

fn configured_shell_name(configured_shell: Option<&str>) -> String {
    configured_shell
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("SHELL").map(PathBuf::from))
        .as_deref()
        .and_then(path_name)
        .map(|name| name.trim_start_matches('-').to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "shell".to_owned())
}

fn path_name(path: &Path) -> Option<String> {
    path.file_name()
        .or_else(|| (!path.as_os_str().is_empty()).then_some(path.as_os_str()))
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
}

/// 把 RGBA 转换回 alacritty 的 RGB（OSC 颜色查询应答用）。
fn to_vte_rgb(rgba: gpui::Rgba) -> Rgb {
    Rgb {
        r: (rgba.r * 255.) as u8,
        g: (rgba.g * 255.) as u8,
        b: (rgba.b * 255.) as u8,
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.kill_current_process();
        self.pty_tx.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::{
        event::VoidListener,
        grid::Scroll as AlacScroll,
        term::{Term, test::mock_term},
    };

    use super::*;

    fn content_of(term: &Term<VoidListener>) -> Content {
        alacritty::make_content(term, None)
    }

    /// 快照基本结构：单行文本的单元格与坐标。
    #[test]
    fn content_snapshot_basic() {
        let term = mock_term("hello");
        let content = content_of(&term);
        assert_eq!(content.cells.len(), 5);
        // 内容从第 0 行开始，逐列排列。
        assert_eq!(content.cells[0].point.line, 0);
        assert_eq!(content.cells[0].point.column, 0);
        assert_eq!(content.cells[0].cell.character(), 'h');
        assert_eq!(content.cells[4].cell.character(), 'o');
        assert_eq!(content.columns, 5);
        assert_eq!(content.screen_lines, 1);
        assert!(content.scrolled_to_bottom);
        assert!(content.scrolled_to_top);
    }

    /// 光标与内容的绝对坐标换算：mock_term 直接写网格，光标保持初始位置 (0, 0)。
    #[test]
    fn cursor_absolute_coordinates() {
        let term = mock_term("hi\n");
        let content = content_of(&term);
        let cursor = content.cursor;
        assert_eq!(cursor.point.line, 0);
        assert_eq!(cursor.point.column, 0);
        assert_eq!(content.cursor_cell.character(), 'h');
    }

    /// 宽字符：WIDE_CHAR 与 WIDE_CHAR_SPACER 标记保留。
    #[test]
    fn wide_char_cells() {
        let term = mock_term("你好");
        let content = content_of(&term);
        let cells: Vec<_> = content.cells.iter().collect();
        assert_eq!(cells.len(), 4);
        assert_eq!(cells[0].cell.character(), '你');
        assert!(cells[0].cell.is_wide_char());
        assert!(cells[1].cell.is_wide_char_spacer());
    }

    /// 滚动映射往返。
    #[test]
    fn scroll_mapping() {
        assert!(matches!(
            Scroll::Delta(3).to_alacritty(),
            AlacScroll::Delta(3)
        ));
        assert!(matches!(Scroll::Bottom.to_alacritty(), AlacScroll::Bottom));
    }

    /// Modes 从 alacritty 模式映射：默认模式应包含 SHOW_CURSOR 等。
    #[test]
    fn modes_from_alacritty() {
        let mode = alacritty_terminal::term::TermMode::default();
        let modes = Modes::from_alacritty(mode);
        assert!(modes.contains(Modes::SHOW_CURSOR));
        assert!(modes.contains(Modes::LINE_WRAP));
        assert!(!modes.contains(Modes::ALT_SCREEN));
    }

    /// 像素尺寸换算：行/列数向下取整并容忍浮点误差。
    #[test]
    fn terminal_bounds_dims() {
        let bounds = TerminalBounds::new(
            Pixels::from(8.),
            Pixels::from(16.),
            Size {
                width: Pixels::from(100.),
                height: Pixels::from(50.),
            },
        );
        assert_eq!(bounds.num_columns(), 12);
        assert_eq!(bounds.num_lines(), 3);
    }

    /// 选择范围包含判断。
    #[test]
    fn selection_range_contains() {
        let range = SelectionRange {
            start: Point { line: 0, column: 1 },
            end: Point { line: 1, column: 3 },
            is_block: false,
        };
        assert!(range.contains(Point { line: 0, column: 1 }));
        assert!(range.contains(Point { line: 1, column: 3 }));
        assert!(!range.contains(Point { line: 0, column: 0 }));
        assert!(!range.contains(Point { line: 2, column: 0 }));
    }
}

#[cfg(all(test, unix))]
mod pty_tests {
    use std::time::{Duration, Instant};

    use gpui::{
        Context, Entity, EntityInputHandler, IntoElement, KeyBinding, Render, TestAppContext,
        VisualTestContext, Window, div, prelude::*, px,
    };
    use zcv_actions::Interrupt;

    use super::*;
    use crate::TerminalView;

    #[derive(Default)]
    struct EmptyView;

    impl Render for EmptyView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn build_terminal(cx: &mut TestAppContext) -> Entity<Terminal> {
        cx.new(|cx| TerminalBuilder::new().build(cx).expect("启动终端失败"))
    }

    /// 刷新渲染快照并断言内容满足条件；轮询等待真实 PTY 输出。
    async fn wait_for_content(
        cx: &mut VisualTestContext,
        terminal: &Entity<Terminal>,
        mut predicate: impl FnMut(&Content) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let matched = cx.update(|window, cx| {
                terminal.update(cx, |t, cx| t.sync(window, cx));
                terminal
                    .read(cx)
                    .last_content()
                    .map(&mut predicate)
                    .unwrap_or(false)
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

    /// 等待 PTY 的前台进程组发生变化，确保命令已经真正接管终端。
    async fn wait_for_foreground_process(
        cx: &mut VisualTestContext,
        terminal: &Entity<Terminal>,
        shell_process_group: sysinfo::Pid,
    ) {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let foreground_pid =
                cx.update(|_window, cx| terminal.read(cx).process_info.foreground_pid());
            if foreground_pid.is_some_and(|pid| pid != shell_process_group) {
                return;
            }
            assert!(Instant::now() < deadline, "等待前台进程启动超时");
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

    /// 真 PTY 回环：写入命令后 shell 输出应出现在快照中。
    #[gpui::test]
    async fn pty_echo_roundtrip(cx: &mut TestAppContext) {
        let terminal = build_terminal(cx);
        let (_, cx) = cx.add_window_view(|_window, _cx| EmptyView);

        cx.update(|_window, cx| {
            terminal.update(cx, |t, cx| {
                t.write_input(b"echo zcv-terminal-ok\n".to_vec(), cx);
            });
        });

        wait_for_content(cx, &terminal, |content| {
            all_text(content).contains("zcv-terminal-ok")
        })
        .await;
    }

    /// 终端上下文的 Ctrl-C 必须中断前台进程，并让 shell 继续接收后续命令。
    #[gpui::test]
    async fn ctrl_c_interrupts_foreground_process(cx: &mut TestAppContext) {
        let terminal = build_terminal(cx);
        let terminal_for_view = terminal.clone();
        let (view, cx) = cx.add_window_view(move |_window, cx| {
            cx.bind_keys([KeyBinding::new("ctrl-c", Interrupt, Some("Terminal"))]);
            TerminalView::new(terminal_for_view, cx)
        });
        cx.update(|window, cx| {
            window.focus(&view.read(cx).focus_handle());
            let _ = window.draw(cx);
            terminal.update(cx, |terminal, cx| {
                terminal.write_input(b"printf 'zcv-%s\\n' shell-ready\n".to_vec(), cx);
            });
        });

        wait_for_content(cx, &terminal, |content| {
            all_text(content).contains("zcv-shell-ready")
        })
        .await;
        let shell_process_group = cx
            .update(|_window, cx| terminal.read(cx).process_info.foreground_pid())
            .expect("shell 应持有 PTY 前台进程组");
        cx.update(|_window, cx| {
            terminal.update(cx, |terminal, cx| {
                terminal.write_input(b"sleep 30\n".to_vec(), cx);
            });
        });
        wait_for_foreground_process(cx, &terminal, shell_process_group).await;

        cx.simulate_keystrokes("ctrl-c");
        cx.update(|_window, cx| {
            terminal.update(cx, |terminal, cx| {
                terminal.write_input(b"printf 'zcv-%s\\n' ctrl-c-ok\n".to_vec(), cx);
            });
        });

        wait_for_content(cx, &terminal, |content| {
            all_text(content).contains("zcv-ctrl-c-ok")
        })
        .await;
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
                t.sync(window, cx);
            });
            terminal
                .read(cx)
                .last_content()
                .is_some_and(|content| content.selection.is_some())
        });
        assert!(has_selection, "设置选择后内容快照应携带选择");

        let cleared = cx.update(|window, cx| {
            terminal.update(cx, |t, cx| {
                t.write_input(Vec::new(), cx);
                t.sync(window, cx);
            });
            terminal
                .read(cx)
                .last_content()
                .is_some_and(|content| content.selection.is_none())
        });
        assert!(cleared, "清除选择后内容快照不应再有选择");
    }
}
