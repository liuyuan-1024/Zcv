//! 底部终端面板：MVP 单终端，首次激活时懒创建。

use gpui::{App, Context, Entity, FocusHandle, Window, prelude::*};
use zcv_workspace::Panel;

use crate::{TerminalBuilder, TerminalView};

pub struct TerminalPanel {
    focus: FocusHandle,
    terminal_view: Option<Entity<TerminalView>>,
}

impl TerminalPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        TerminalPanel {
            focus: cx.focus_handle(),
            terminal_view: None,
        }
    }

    /// 创建终端：工作目录取当前项目根（ActiveProjectRoot），shell 取用户设置。
    fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = cx
            .try_global::<zcv_project::ActiveProjectRoot>()
            .and_then(|root| root.0.clone());
        // PTY 创建失败（fork/exec 异常）属于严重错误，直接终止应用并给出原因。
        let terminal = cx.new(|cx| {
            TerminalBuilder::new()
                .set_cwd(cwd)
                .build(cx)
                .unwrap_or_else(|error| panic!("创建终端失败：{error}"))
        });
        let view = cx.new(|cx| TerminalView::new(terminal, cx));
        // Dock 只聚焦面板句柄，这里补聚焦到终端视图，键盘才能直接输入。
        let focus = view.read(cx).focus_handle();
        window.focus(&focus);
        self.terminal_view = Some(view);
    }

    fn focus_view(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = &self.terminal_view {
            let focus = view.read(cx).focus_handle();
            window.focus(&focus);
        }
    }
}

impl Panel for TerminalPanel {
    fn icon() -> &'static str {
        "icons/terminal.svg"
    }

    fn label() -> &'static str {
        "终端"
    }

    fn persistent_name() -> &'static str {
        "terminal"
    }

    fn focus_handle(&self, cx: &App) -> FocusHandle {
        // 无终端时返回面板自身句柄，保证 FocusOrHidePanel 的聚焦路径始终有效。
        self.terminal_view
            .as_ref()
            .map(|view| view.read(cx).focus_handle())
            .unwrap_or_else(|| self.focus.clone())
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if active {
            if self.terminal_view.is_none() {
                self.new_terminal(window, cx);
            } else {
                self.focus_view(window, cx);
            }
        }
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.terminal_view {
            Some(view) => div().size_full().child(view.clone()),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .track_focus(&self.focus)
                .key_context("terminal")
                .tab_index(0)
                .text_color(zcv_theme::color::current(cx).text_placeholder)
                .child("终端未启动"),
        }
    }
}

use gpui::{IntoElement, Render, div};
