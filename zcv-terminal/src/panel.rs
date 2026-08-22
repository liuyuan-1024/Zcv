//! 底部终端面板：内嵌 Pane，终端以 Item 形式打开。
//!
//! 对齐 Zed：面板复用编辑区的 Pane，多终端标签栏、tab 切换、关闭与编辑区同构。

use gpui::{App, ClickEvent, Context, Entity, FocusHandle, Window, prelude::*};
use zcv_actions::NewTerminal;
use zcv_ui::Glyph;
use zcv_workspace::{Pane, Panel};

use crate::{TerminalBuilder, TerminalView};

pub struct TerminalPanel {
    pane: Entity<Pane>,
}

impl TerminalPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let pane = cx.new(Pane::new);
        let weak = cx.weak_entity();
        let pane_for_button = pane.clone();
        pane.update(cx, |pane, _| {
            pane.set_tab_bar_trailing(move |_cx: &App| {
                let weak_for_click = weak.clone();
                Glyph::icon(("terminal-new-terminal", 0usize), "icons/plus.svg")
                    .label("新建终端")
                    .shortcut(&NewTerminal, _cx)
                    .on_click(move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        if let Some(panel) = weak_for_click.upgrade() {
                            panel.update(cx, |panel, cx| {
                                panel.new_terminal(window, cx);
                            });
                        }
                    })
                    .into_any_element()
            });
        });
        TerminalPanel {
            pane: pane_for_button,
        }
    }

    /// 创建终端：工作目录取当前项目根（ActiveProjectRoot），shell 取用户设置。
    /// 面板激活懒创建与外部新建终端命令共用。
    pub fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        // 终端作为 Item 打开进 Pane，焦点直接落在终端视图上。
        let focus = self.pane.update(cx, |pane, cx| {
            pane.open_item(Box::new(view), false, window, cx)
        });
        window.focus(&focus);
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
        // 优先聚焦当前终端；无终端时回退到 Pane 自身句柄，保证 FocusOrHidePanel 的聚焦路径始终有效。
        self.pane
            .read(cx)
            .active_item()
            .map(|item| item.item_focus_handle(cx))
            .unwrap_or_else(|| self.pane.read(cx).focus_handle())
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if active {
            if self.pane.read(cx).active_item().is_some() {
                // Dock 只聚焦面板句柄，这里补聚焦到当前终端。
                let focus = self.focus_handle(cx);
                window.focus(&focus);
            } else {
                self.new_terminal(window, cx);
            }
        }
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.pane.clone())
    }
}

use gpui::{IntoElement, Render, div};
