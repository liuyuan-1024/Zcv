//! 底部终端面板：内嵌 Pane，终端以 Item 形式打开。
//!
//! 面板复用编辑区的 Pane，多终端标签栏、tab 切换、关闭与编辑区同构。

use gpui::{
    App, ClickEvent, Context, Entity, EntityId, EventEmitter, FocusHandle, Subscription, Window,
    prelude::*,
};
use serde::{Deserialize, Serialize};
use zcv_actions::NewTerminal;
use zcv_project::Project;
use zcv_ui::Button;
use zcv_workspace::{Pane, PaneEvent, Panel, PanelEvent};

/// 终端会话快照：重建 PTY 所需的最小信息。
#[derive(Debug, Serialize, Deserialize)]
struct SerializedTerminal {
    /// 会话保存时的工作目录。
    cwd: Option<std::path::PathBuf>,
}

use crate::{TerminalBuilder, TerminalView};

pub struct TerminalPanel {
    project: Entity<Project>,
    pane: Entity<Pane>,
    _subscriptions: Vec<Subscription>,
    /// 首次渲染时注册带 window 的订阅（构造函数中没有 Window）。
    initialized: bool,
    /// 恢复会话进行中：抑制清空终端触发的 PaneEvent::Remove（防面板误折叠）。
    restoring: bool,
}

impl EventEmitter<PanelEvent> for TerminalPanel {}

impl TerminalPanel {
    pub fn new(project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let pane = cx.new(Pane::new);
        let weak = cx.weak_entity();
        let pane_for_button = pane.clone();
        pane.update(cx, |pane, _| {
            pane.set_tab_bar_trailing(move |_cx: &App| {
                let weak_for_click = weak.clone();
                Button::icon(("terminal-new-terminal", 0usize), "icons/plus.svg")
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
            project,
            pane: pane_for_button,
            _subscriptions: Vec::new(),
            initialized: false,
            restoring: false,
        }
    }

    /// 创建终端：工作目录取所属 Project 的当前根，shell 取用户设置。
    /// 面板激活懒创建与外部新建终端命令共用。
    pub fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cwd = project_terminal_cwd(self.project.read(cx));
        self.new_terminal_with_cwd(cwd, window, cx);
    }

    /// 以指定工作目录创建终端（恢复会话时沿用保存的 cwd）。
    fn new_terminal_with_cwd(
        &mut self,
        cwd: Option<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        window.focus(&focus, cx);
    }
}

fn project_terminal_cwd(project: &Project) -> Option<std::path::PathBuf> {
    project.root().map(std::path::Path::to_path_buf)
}

#[cfg(all(test, unix))]
#[path = "test/panel.rs"]
mod tests;

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
                window.focus(&focus, cx);
            } else {
                self.new_terminal(window, cx);
            }
        }
    }

    /// 序列化终端会话列表（工作目录）；始终保存（空列表也持久化，恢复时为空转）。
    fn serialized_state(&self, cx: &App) -> Option<serde_json::Value> {
        let items: Vec<SerializedTerminal> = self
            .pane
            .read(cx)
            .tabs()
            .iter()
            .filter_map(|item| {
                let view = item.act_as::<TerminalView>(cx)?;
                let cwd = view
                    .read(cx)
                    .terminal
                    .read(cx)
                    .working_directory()
                    .map(|path| path.to_path_buf());
                Some(SerializedTerminal { cwd })
            })
            .collect();
        serde_json::to_value(&items).ok()
    }

    /// 恢复终端会话：按保存顺序重建 PTY（shell 会话为新进程，沿用保存的工作目录）。
    fn restore_state(
        &mut self,
        state: serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(items) = serde_json::from_value::<Vec<SerializedTerminal>>(state) else {
            return;
        };
        self.restoring = true;
        // 记录恢复前已存在的终端（dock 恢复可见性时懒创建的），重建后清理。
        // 先重建再清理：清理时 tabs 非空，不会触发 PaneEvent::Remove 误折叠面板（事件异步分发，同步的 restoring 标志无法抑制）。
        let preexisting: Vec<EntityId> = self
            .pane
            .read(cx)
            .tabs()
            .iter()
            .map(|item| item.item_id())
            .collect();
        for item in items {
            self.new_terminal_with_cwd(item.cwd, window, cx);
        }
        for id in preexisting {
            self.pane
                .update(cx, |pane, cx| pane.close_tab(id, window, cx));
        }
        self.restoring = false;
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 首次渲染注册带 window 的订阅（构造函数中没有 Window）。
        if !self.initialized {
            self.initialized = true;
            let subscription = cx.subscribe_in(
                &self.pane,
                window,
                |this, _, event: &PaneEvent, _window, cx| {
                    // 会话增删时通知宿主保存布局（恢复期间不触发，避免恢复过程反复落盘）。
                    if !this.restoring
                        && matches!(
                            event,
                            PaneEvent::AddItem { .. } | PaneEvent::RemovedItem { .. }
                        )
                    {
                        cx.emit(PanelEvent::StateChanged);
                    }
                    // 最后一个终端关闭（Pane 请求移除自身）时请求关闭面板；
                    // 宿主订阅 PanelEvent::Close 统一折叠（恢复期间不触发）。
                    if matches!(event, PaneEvent::Remove) && !this.restoring {
                        cx.emit(PanelEvent::Close);
                    }
                },
            );
            self._subscriptions.push(subscription);
        }
        div().size_full().child(self.pane.clone())
    }
}

use gpui::{IntoElement, Render, div};
