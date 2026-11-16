//! 底部终端面板：内嵌 Pane，终端以 Item 形式打开。
//!
//! 面板复用编辑区的 Pane，多终端标签栏、tab 切换、关闭与编辑区同构。

use std::{path::PathBuf, time::Duration};

use gpui::{
    App, ClickEvent, Context, Entity, EntityId, EventEmitter, FocusHandle, IntoElement, Render,
    Subscription, WeakEntity, Window, div, prelude::*,
};
use serde::{Deserialize, Serialize};
use zcv_actions::NewTerminal;
use zcv_project::Project;
use zcv_ui::{Button, ToastKind};
use zcv_workspace::{Pane, PaneEvent, Panel, PanelEvent, Workspace};

/// 终端会话快照：重建 PTY 所需的最小信息。
#[derive(Debug, Serialize, Deserialize)]
struct SerializedTerminal {
    /// 会话保存时的工作目录。
    cwd: Option<PathBuf>,
}

use crate::{TerminalBuilder, TerminalView};

pub struct TerminalPanel {
    project: Entity<Project>,
    workspace: WeakEntity<Workspace>,
    pane: Entity<Pane>,
    focus: FocusHandle,
    _pane_subscription: Subscription,
}

impl EventEmitter<PanelEvent> for TerminalPanel {}

impl TerminalPanel {
    pub fn new(
        project: Entity<Project>,
        workspace: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Self {
        let pane = cx.new(|cx| Pane::new(project.clone(), cx));
        let focus = cx.focus_handle();
        let weak = cx.weak_entity();
        let workspace_for_button = workspace.clone();
        pane.update(cx, |pane, _| {
            pane.set_tab_bar_trailing(move |cx: &App| {
                let weak_for_click = weak.clone();
                let workspace_for_click = workspace_for_button.clone();
                Button::icon(("terminal-new-terminal", 0usize), "icons/plus.svg")
                    .label("新建终端")
                    .shortcut(zcv_keymap::display_shortcut(&NewTerminal, cx))
                    .on_click(move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        if let Some(panel) = weak_for_click.upgrade() {
                            let created =
                                panel.update(cx, |panel, cx| panel.new_terminal(window, cx));
                            if created && let Some(workspace) = workspace_for_click.upgrade() {
                                workspace.update(cx, |workspace, cx| {
                                    workspace.open_panel("terminal", window, cx);
                                });
                            }
                        }
                    })
                    .into_any_element()
            });
        });
        let pane_subscription = cx.subscribe(&pane, |panel, pane, event, cx| match event {
            PaneEvent::AddItem { .. } => cx.emit(PanelEvent::StateChanged),
            PaneEvent::RemovedItem { .. } => {
                cx.emit(PanelEvent::StateChanged);
                if pane.read(cx).tabs().is_empty() {
                    cx.emit(PanelEvent::Close);
                }
            }
            PaneEvent::ItemError { message } => panel.report_error(message.clone(), cx),
            PaneEvent::ActivateItem { .. } => {}
        });
        TerminalPanel {
            project,
            workspace,
            pane,
            focus,
            _pane_subscription: pane_subscription,
        }
    }

    /// 创建终端：工作目录取所属 Project 的当前根，shell 取用户设置。
    /// 面板激活懒创建与外部新建终端命令共用。
    pub fn new_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let cwd = project_terminal_cwd(self.project.read(cx));
        match self.new_terminal_with_cwd(cwd, window, cx) {
            Ok(()) => true,
            Err(error) => {
                self.report_error(format!("创建终端失败：{error:#}"), cx);
                false
            }
        }
    }

    /// 以指定工作目录创建终端（恢复会话时沿用保存的 cwd）。
    fn new_terminal_with_cwd(
        &mut self,
        cwd: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let terminal = TerminalBuilder::new().set_cwd(cwd).build(cx)?;
        let terminal = cx.new(|cx| terminal.activate(cx));
        let view = cx.new(|cx| TerminalView::new(terminal, cx));
        self.pane.update(cx, |pane, cx| {
            pane.open_item(Box::new(view), false, window, cx)
        });
        Ok(())
    }

    fn report_error(&self, message: String, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        // 面板可能由 Workspace 命令或 Dock 激活回调调用，须等当前实体借用结束后展示。
        cx.defer(move |cx| {
            workspace
                .update(cx, |workspace, cx| {
                    workspace.show_toast(
                        ToastKind::Error,
                        message,
                        vec![],
                        Some(Duration::from_secs(5)),
                        cx,
                    );
                })
                .ok();
        });
    }
}

fn project_terminal_cwd(project: &Project) -> Option<PathBuf> {
    // 多根工作区以主根（首个根）作为终端默认工作目录。
    project.main_root().map(std::path::Path::to_path_buf)
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

    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }

    fn activation_focus_handle(&self, cx: &App) -> FocusHandle {
        let pane = self.pane.read(cx);
        pane.active_item()
            .map(|item| item.item_focus_handle(cx))
            .unwrap_or_else(|| pane.focus_handle())
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if active && self.pane.read(cx).active_item().is_none() {
            self.new_terminal(window, cx);
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
        // 先恢复会话，成功后再移除 Dock 激活时懒创建的终端。
        let preexisting: Vec<EntityId> = self
            .pane
            .read(cx)
            .tabs()
            .iter()
            .map(|item| item.item_id())
            .collect();
        let was_empty = items.is_empty();
        let mut restored = false;
        let mut first_error = None;
        let mut failures = 0;
        for item in items {
            match self.new_terminal_with_cwd(item.cwd, window, cx) {
                Ok(()) => restored = true,
                Err(error) => {
                    failures += 1;
                    first_error.get_or_insert(error);
                }
            }
        }
        if restored || was_empty {
            self.pane.update(cx, |pane, cx| {
                for id in preexisting {
                    pane.remove_tab(id, window, cx);
                }
            });
        }
        if let Some(error) = first_error {
            self.report_error(format!("恢复 {failures} 个终端会话失败：{error:#}"), cx);
        }
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.focus)
            .size_full()
            .child(self.pane.clone())
    }
}
