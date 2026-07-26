//! PanelButtons —— 底栏按钮组。
//!
//! 每个 DockPosition 一个按钮组，持有对应的 Dock Entity 来查询面板激活状态，激活时高亮显示。
//! 参考 Zed `crates/workspace/src/dock.rs`。

use gpui::{App, Context, ElementId, Entity, Render, Subscription, Window, div, prelude::*};

use super::{Dock, DockPosition, StatusItemView};
use crate::editor::Editor;
use crate::theme::{color, space};
use crate::ui::Glyph;

/// 面板点击调度函数：将点击转为 gpui action dispatch。
pub(crate) type PanelDispatch = fn(&mut Window, &mut App);

/// 底栏按钮组：绑定一个 Dock Entity，渲染其所有面板。
pub(crate) struct PanelButtons {
    dock: Entity<Dock>,
    /// 与 dock.panels 顺序一一对应的 dispatch 函数。
    dispatches: Vec<PanelDispatch>,
    _subscription: Subscription,
}

impl PanelButtons {
    pub(crate) fn new(
        dock: Entity<Dock>,
        dispatches: Vec<PanelDispatch>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Dock 状态变化时自动重绘
        let sub = cx.observe(&dock, |_, _, cx| cx.notify());
        Self {
            dock,
            dispatches,
            _subscription: sub,
        }
    }
}

impl StatusItemView for PanelButtons {
    fn set_active_editor(&mut self, _editor: Option<&Entity<Editor>>, _cx: &mut Context<Self>) {
        // PanelButtons 不追踪 Editor 状态。
    }
}

impl Render for PanelButtons {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let dock = self.dock.read(cx);
        if dock.panels.is_empty() || self.dispatches.is_empty() {
            return div();
        }

        let active_index = dock.active_panel_index();
        let is_open = dock.is_open;
        let area = dock.position;

        let buttons: Vec<_> = dock
            .panels
            .iter()
            .enumerate()
            .zip(&self.dispatches)
            .map(|((i, handle), dispatch)| {
                let icon_path = handle.icon();
                let label = handle.label();
                let action = handle.action_name();
                let is_active = Some(i) == active_index && is_open;
                let fg = if is_active {
                    color::highlight()
                } else {
                    color::default()
                };
                let on_click = *dispatch;

                Glyph::icon(ElementId::Name(icon_path.into()), icon_path)
                    .label(label)
                    .shortcut_by_name(action, cx)
                    .color(fg)
                    .on_click(on_click)
                    .into_any_element()
            })
            .collect();

        let divider = div()
            .w(gpui::px(1.0))
            .h_full()
            .bg(color::current().gray.s[4]);

        match area {
            DockPosition::Left => div()
                .flex()
                .items_center()
                .gap(space::S6)
                .children(buttons)
                .child(divider),
            DockPosition::Right | DockPosition::Bottom => div()
                .flex()
                .items_center()
                .gap(space::S6)
                .child(divider)
                .children(buttons),
        }
    }
}
