//! PanelButtons —— 底栏按钮组。
//!
//! 每个 DockPosition 一个按钮组，持有对应的 Dock Entity 来查询面板激活状态，激活时高亮显示。
//! 参考 Zed `crates/workspace/src/dock.rs`。

use gpui::{
    App, ClickEvent, Context, ElementId, Entity, Render, Subscription, Window, div, prelude::*,
};

use crate::ItemHandle;
use crate::{
    dock::{Dock, DockPosition},
    status_bar::StatusItemView,
};
use zcv_theme::{color, space};
use zcv_ui::Glyph;

/// 底栏按钮组：绑定一个 Dock Entity，渲染其所有面板。
pub struct PanelButtons {
    dock: Entity<Dock>,
    _subscription: Subscription,
}

impl PanelButtons {
    pub fn new(dock: Entity<Dock>, cx: &mut Context<Self>) -> Self {
        // Dock 状态变化时自动重绘
        let sub = cx.observe(&dock, |_, _, cx| cx.notify());
        Self {
            dock,
            _subscription: sub,
        }
    }
}

impl StatusItemView for PanelButtons {
    fn set_active_pane_item(&mut self, _item: Option<&dyn ItemHandle>, _cx: &mut Context<Self>) {
        // PanelButtons 不追踪 Editor 状态。
    }
}

impl Render for PanelButtons {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let dock = self.dock.read(cx);
        if dock.panels.is_empty() {
            return div();
        }

        let active_index = dock.active_panel_index();
        let is_open = dock.is_open;
        let area = dock.position;

        let buttons: Vec<_> = dock
            .panels
            .iter()
            .enumerate()
            .map(|(i, handle)| {
                let icon_path = handle.icon();
                let label = handle.label();
                let is_active = Some(i) == active_index && is_open;
                let fg = if is_active {
                    color::current(cx).icon_accent
                } else {
                    color::current(cx).text
                };
                let action = handle.toggle_action(cx);
                let on_click = {
                    let handle = handle.clone();
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        window.dispatch_action(handle.toggle_action(cx), cx);
                    }
                };

                Glyph::icon(ElementId::Name(icon_path.into()), icon_path)
                    .label(label)
                    .shortcut(action.as_ref(), cx)
                    .color(fg)
                    .on_click(on_click)
                    .into_any_element()
            })
            .collect();

        let divider = div()
            .w(gpui::px(1.0))
            .h_full()
            .bg(color::current(cx).border_variant);

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
