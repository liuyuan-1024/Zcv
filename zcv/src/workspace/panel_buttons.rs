//! PanelButtons —— 底栏中不追踪 Editor 状态的静态按钮组。
//!
//! 每个 PanelButtons 绑定一个 DockArea，遍历该区域的面板注册表生成按钮。
//! 面板数据来自 `default_panels()`。

use gpui::{Context, Entity, Render, Window, div, prelude::*};

use crate::editor::editor::Editor;
use crate::theme::{color, space};
use crate::ui::glyph::Glyph;
use crate::workspace::dock::{DockArea, PanelEntry, default_panels};
use crate::workspace::status_bar::StatusItemView;

/// 底栏按钮组 —— 绑定一个 Dock 区域，遍历该区域的 panel_entries 生成按钮。
pub(crate) struct PanelButtons {
    area: DockArea,
    entries: Vec<(usize, PanelEntry)>,
}

impl PanelButtons {
    pub(crate) fn new(area: DockArea) -> Self {
        let entries = default_panels()
            .into_iter()
            .enumerate()
            .filter(|(_, p)| p.dock_area == area)
            .collect();
        Self { area, entries }
    }
}

impl StatusItemView for PanelButtons {
    fn set_active_editor(&mut self, _editor: Option<&Entity<Editor>>, _cx: &mut Context<Self>) {
        // PanelButtons 不追踪 Editor 状态。
    }
}

impl Render for PanelButtons {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        if self.entries.is_empty() {
            return div();
        }

        let buttons: Vec<_> = self
            .entries
            .iter()
            .map(|&(i, ref entry)| {
                let dispatch = entry.dispatch;
                Glyph::icon(("panel-btn", i as u64), entry.icon)
                    .label(entry.label)
                    .shortcut_by_name(entry.action_name, cx)
                    .color(color::default())
                    .on_click(move |window, cx| dispatch(window, cx))
                    .into_any_element()
            })
            .collect();

        // 左 dock 按钮组右侧加分隔线，右/底 dock 按钮组左侧加分隔线
        let divider = div()
            .w(gpui::px(1.0))
            .h_full()
            .bg(color::current().gray.s[4]);

        match self.area {
            DockArea::Left => div()
                .flex()
                .items_center()
                .gap(space::S6)
                .children(buttons)
                .child(divider),
            DockArea::Right | DockArea::Bottom => div()
                .flex()
                .items_center()
                .gap(space::S6)
                .child(divider)
                .children(buttons),
        }
    }
}
