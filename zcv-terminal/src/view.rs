//! 终端视图：焦点管理、键盘/滚轮输入与渲染元素的组装。

use std::ops::Range;

use gpui::{
    App, Bounds, Context, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    IntoElement, KeyDownEvent, Pixels, Render, ScrollWheelEvent, SharedString, Subscription,
    UTF16Selection, Window, div, prelude::*, px,
};

use crate::{
    Event, Modes, Point, SelectionType, Terminal, TerminalSettings,
    element::TerminalElement,
    mappings::{keys, mouse},
};

use std::cell::Cell;
use std::time::{Duration, Instant};

use zcv_actions::{Clear, Copy, Interrupt, Paste};
use zcv_theme::space;
use zcv_workspace::{Item, ItemEvent};

/// 拖拽选择自动滚动的限频间隔（≈60Hz，与编辑器 drag_autoscroll 同款）。
const AUTOSCROLL_INTERVAL: Duration = Duration::from_millis(16);

pub(crate) struct TerminalView {
    pub(crate) terminal: Entity<Terminal>,
    focus: FocusHandle,
    pub(crate) focused: bool,
    /// 进行中的拖拽选择：起点与选择类型。
    dragging: Option<(Point, SelectionType)>,
    /// 拖拽选择自动滚动的限频时间戳（跨帧持久；事件频率可远超帧率，滚动频率需封顶）。
    last_drag_autoscroll: Cell<Instant>,
    /// 输入法合成中的 marked 文本。
    ime_marked_text: Option<String>,
    /// 光标格的像素 bounds（元素相对坐标），IME 候选窗定位用。
    last_cursor_bounds: Option<Bounds<Pixels>>,
    initialized: bool,
    _subscriptions: Vec<Subscription>,
}

impl TerminalView {
    pub fn new(terminal: Entity<Terminal>, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let mut view = TerminalView {
            terminal,
            focus,
            focused: false,
            dragging: None,
            last_drag_autoscroll: Cell::new(Instant::now() - AUTOSCROLL_INTERVAL),
            ime_marked_text: None,
            last_cursor_bounds: None,
            initialized: false,
            _subscriptions: Vec::new(),
        };
        view.subscribe_terminal_events(cx);
        // 观察 Terminal 的 notify（滚动、选择、输入等触发），保证视图重绘。
        cx.observe(&view.terminal, |_, _, cx| cx.notify()).detach();
        view
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// 终端字体大小（像素）：显式配置优先，缺省跟随编辑器字号。
    pub(crate) fn font_size(&self, cx: &App) -> Pixels {
        px(TerminalSettings::load(cx).font_size)
    }

    /// 终端行高（像素），字体大小 × 行高倍率。
    pub(crate) fn line_height(&self, cx: &App) -> Pixels {
        let settings = TerminalSettings::load(cx);
        px(settings.font_size * settings.line_height)
    }

    /// 输入法组合中的 marked 文本（供渲染预览）。
    pub(crate) fn marked_text(&self) -> Option<&str> {
        self.ime_marked_text.as_deref()
    }

    /// 记录光标格像素位置（渲染元素每帧更新，供 IME 候选窗定位）。
    pub(crate) fn set_ime_cursor_bounds(&mut self, bounds: Bounds<Pixels>) {
        self.last_cursor_bounds = Some(bounds);
    }

    /// 排空终端事件队列并刷新渲染快照（渲染元素每帧调用）。
    pub(crate) fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal
            .update(cx, |terminal, cx| terminal.sync(window, cx));
    }

    fn subscribe_terminal_events(&mut self, cx: &mut Context<Self>) {
        let subscription =
            cx.subscribe(&self.terminal, |_view, _, event: &Event, cx| match event {
                // 标题变化时通知 Pane 刷新标签栏标题。
                Event::TitleChanged(_) => cx.emit(ItemEvent::UpdateTab),
                Event::Wakeup | Event::Bell | Event::SelectionsChanged => {
                    cx.notify();
                }
            });
        self._subscriptions.push(subscription);
    }

    fn set_focused(&mut self, focused: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.focused = focused;
        self.terminal
            .update(cx, |terminal, cx| terminal.focus_terminal(focused, cx));
        cx.notify();
    }

    /// 光标是否绘制：与焦点绑定（失焦隐藏；聚焦恒显示，终端光标不闪烁）。
    /// 实时查询焦点状态。
    pub(crate) fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    pub(crate) fn should_show_cursor(&self, focused: bool, _cx: &App) -> bool {
        focused
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mode = self
            .terminal
            .read(cx)
            .last_content()
            .map(|content| content.mode)
            .unwrap_or(Modes::NONE);
        let option_as_meta = TerminalSettings::load(cx).option_as_meta;
        if let Some(input) = keys::to_esc_str(&event.keystroke, &mode, option_as_meta) {
            self.terminal.update(cx, |terminal, cx| {
                terminal.write_input(input.into_bytes(), cx)
            });
            // 已作为终端输入处理的键停止事件传播：否则平台会继续走文本输入通道（replace_text_in_range）导致同一按键写入两次（空格、alt+字符等）。
            cx.stop_propagation();
        }
    }

    /// 复制选择到系统剪贴板（复用 editor::Copy 动作名，keymap 与编辑器一致）。
    fn handle_copy(&mut self, _: &Copy, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal
            .update(cx, |terminal, cx| terminal.copy(window, cx));
    }

    /// 粘贴剪贴板内容到终端（复用 editor::Paste 动作名）。
    fn handle_paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal
            .update(cx, |terminal, cx| terminal.paste(window, cx));
    }

    /// 清空屏幕与滚动回看。
    fn handle_clear(&mut self, _: &Clear, _window: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, cx| terminal.clear(cx));
    }

    /// 向前台进程发送 ETX（0x03），由终端驱动解释为 SIGINT。
    fn handle_interrupt(&mut self, _: &Interrupt, _window: &mut Window, cx: &mut Context<Self>) {
        self.terminal
            .update(cx, |terminal, cx| terminal.write_input(vec![0x03], cx));
        cx.stop_propagation();
    }

    /// 鼠标按下：报告模式转发字节；否则开始选择（双击语义选择、三击整行）。
    pub(crate) fn handle_mouse_down(
        &mut self,
        event: &gpui::MouseDownEvent,
        point: crate::Point,
        side: crate::SelectionSide,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(content) = self.terminal.read(cx).last_content().cloned() else {
            return;
        };
        let mode = content.mode;
        let display_offset = content.display_offset;
        let screen_lines = content.screen_lines;

        if mode.intersects(Modes::MOUSE_MODE) {
            if let Some(bytes) = mouse::mouse_button_report(
                event.button,
                &event.modifiers,
                point,
                display_offset,
                screen_lines,
                &mode,
                true,
            ) {
                self.terminal
                    .update(cx, |terminal, _| terminal.write_to_pty(bytes));
            }
            return;
        }
        if event.button != gpui::MouseButton::Left {
            return;
        }
        let ty = match event.click_count {
            2 => SelectionType::Semantic,
            3 => SelectionType::Lines,
            _ => SelectionType::Simple,
        };
        self.dragging = Some((point, ty));
        self.terminal.update(cx, |terminal, cx| {
            terminal.select_range(ty, crate::SelectionPoint { point, side }, None, cx);
        });
        window.prevent_default();
    }

    /// 鼠标移动：拖拽中更新选择；报告模式转发移动字节。
    ///
    /// `autoscroll` 为拖拽选择期间鼠标在视口边缘外时的事件滚动量（像素，正 = 回看历史）；
    /// 先滚动视口再用钳制后的网格坐标更新选区，选区随视口持续扩展。
    pub(crate) fn handle_mouse_move(
        &mut self,
        event: &gpui::MouseMoveEvent,
        point: crate::Point,
        side: crate::SelectionSide,
        autoscroll: Pixels,
        cx: &mut Context<Self>,
    ) {
        let Some(content) = self.terminal.read(cx).last_content().cloned() else {
            return;
        };
        let mode = content.mode;
        let display_offset = content.display_offset;
        let screen_lines = content.screen_lines;

        if mode.intersects(Modes::MOUSE_MODE) {
            if let Some(bytes) = mouse::mouse_moved_report(
                event.pressed_button,
                &event.modifiers,
                point,
                display_offset,
                screen_lines,
                &mode,
            ) {
                self.terminal
                    .update(cx, |terminal, _| terminal.write_to_pty(bytes));
            }
            return;
        }
        if let Some((start, ty)) = self.dragging {
            if autoscroll != Pixels::ZERO
                && self.last_drag_autoscroll.get().elapsed() >= AUTOSCROLL_INTERVAL
            {
                self.last_drag_autoscroll.set(Instant::now());
                let line_height = self.line_height(cx);
                self.terminal.update(cx, |terminal, cx| {
                    terminal.scroll_px(gpui::TouchPhase::Moved, autoscroll, line_height, cx);
                });
            }
            let _ = start;
            let _ = ty;
            self.terminal.update(cx, |terminal, cx| {
                terminal.update_selection(crate::SelectionPoint { point, side }, cx);
            });
        }
    }

    /// 鼠标释放：结束拖拽；报告模式转发释放字节。
    pub(crate) fn handle_mouse_up(
        &mut self,
        event: &gpui::MouseUpEvent,
        point: crate::Point,
        cx: &mut Context<Self>,
    ) {
        self.dragging = None;
        let Some(content) = self.terminal.read(cx).last_content().cloned() else {
            return;
        };
        let mode = content.mode;
        let display_offset = content.display_offset;
        let screen_lines = content.screen_lines;
        if mode.intersects(Modes::MOUSE_MODE)
            && let Some(bytes) = mouse::mouse_button_report(
                event.button,
                &event.modifiers,
                point,
                display_offset,
                screen_lines,
                &mode,
                false,
            )
        {
            self.terminal
                .update(cx, |terminal, _| terminal.write_to_pty(bytes));
        }
    }

    /// 滚轮：鼠标报告模式转报告字节；备用屏幕回退方向键；否则像素滚动。
    pub(crate) fn handle_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        point: Option<crate::Point>,
        scroll_lines: i32,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(content) = self.terminal.read(cx).last_content().cloned() else {
            return;
        };
        let mode = content.mode;
        let line_height = self.line_height(cx);
        let display_offset = content.display_offset;
        let screen_lines = content.screen_lines;

        // 鼠标报告模式：滚轮逐行上报。
        if scroll_lines != 0
            && mode.intersects(Modes::MOUSE_MODE)
            && let Some(point) = point
            && let Some(reports) =
                mouse::scroll_report(scroll_lines, point, display_offset, screen_lines, &mode)
        {
            for report in reports {
                self.terminal.update(cx, |terminal, _| {
                    terminal.write_to_pty(report);
                });
            }
            return;
        }

        // 备用屏幕（vim 等）且开启 alternate scroll：转方向键，按住 shift 时例外。
        if scroll_lines != 0
            && mode.contains(Modes::ALT_SCREEN)
            && mode.contains(Modes::ALTERNATE_SCROLL)
            && !event.modifiers.shift
        {
            self.terminal.update(cx, |terminal, _| {
                terminal.write_to_pty(mouse::alt_scroll(scroll_lines))
            });
            return;
        }

        // 普通滚动：像素累加，保证平滑（touch phase 驱动手势状态）。
        let delta = event.delta.pixel_delta(line_height);
        self.terminal.update(cx, |terminal, cx| {
            terminal.scroll_px(event.touch_phase, delta.y, line_height, cx)
        });
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let text = self.ime_marked_text.as_deref()?;
        actual_range.replace(range_utf16.clone());
        Some(
            text.chars()
                .take(range_utf16.end)
                .skip(range_utf16.start)
                .collect(),
        )
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        None
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.ime_marked_text
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.ime_marked_text = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 输入法确认提交（含无合成时的普通字符输入）：清 marked 并写入终端。
        self.ime_marked_text = None;
        self.terminal.update(cx, |terminal, cx| {
            terminal.write_input(text.as_bytes().to_vec(), cx)
        });
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 组合预览：只更新 marked 文本，不写入终端。
        self.ime_marked_text = Some(new_text.to_string());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // 光标格元素相对位置 + 元素窗口偏移 = 候选窗屏幕位置。
        self.last_cursor_bounds
            .map(|bounds| Bounds::new(bounds.origin + element_bounds.origin, bounds.size))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<ItemEvent> for TerminalView {}

// ═══ Item：终端作为标签页接入 Pane ═══════════════════════════════

impl Item for TerminalView {
    type Event = ItemEvent;

    fn tab_content_text(&self, cx: &App) -> SharedString {
        SharedString::from(self.terminal.read(cx).tab_title())
    }

    fn tab_icon(&self, _cx: &App) -> Option<SharedString> {
        Some(SharedString::from("icons/terminal.svg"))
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 首次渲染时注册焦点事件（构造函数中没有 Window）。
        if !self.initialized {
            let focus = self.focus.clone();
            cx.on_focus(&focus, window, |view, window, cx| {
                view.set_focused(true, window, cx);
            })
            .detach();
            cx.on_blur(&focus, window, |view, window, cx| {
                view.set_focused(false, window, cx);
            })
            .detach();
            cx.observe_window_activation(window, |view, window, cx| {
                let focused = view.focus.is_focused(window);
                view.set_focused(focused, window, cx);
            })
            .detach();
            self.initialized = true;
        }

        div()
            .track_focus(&self.focus)
            .key_context("Terminal")
            .tab_index(0)
            .size_full()
            .overflow_hidden()
            .bg(zcv_theme::color::current(cx).editor_background)
            .px(space::S8)
            .py(space::S4)
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_action(cx.listener(Self::handle_copy))
            .on_action(cx.listener(Self::handle_paste))
            .on_action(cx.listener(Self::handle_clear))
            .on_action(cx.listener(Self::handle_interrupt))
            .child(TerminalElement::new(cx.entity()))
    }
}
