//! PanelButtons —— 底栏中不追踪 Editor 状态的静态按钮组。
//!
//! 每个 PanelButtons 绑定一个 DockArea，遍历 LayoutController 中该区域的面板注册表生成按钮。
//! 面板身份由注册表 index 标识。

use gpui::{Context, Entity, Render, Window, div, prelude::*};

use crate::editor::editor::Editor;
use crate::theme::{color, space};
use crate::ui::glyph::Glyph;
use crate::workspace::dock::{DockArea, LayoutRef};
use crate::workspace::status_bar::StatusItemView;

/// 底栏按钮组 —— 绑定一个 Dock 区域，遍历该区域的 panel_entries 生成按钮。
pub(crate) struct PanelButtons {
    area: DockArea,
}

impl PanelButtons {
    pub(crate) fn new(area: DockArea) -> Self {
        Self { area }
    }
}

impl StatusItemView for PanelButtons {
    fn set_active_editor(&mut self, _editor: Option<&Entity<Editor>>, _cx: &mut Context<Self>) {
        // PanelButtons 不追踪 Editor 状态；按钮高亮在 Render 中通过 LayoutRef 判断。
    }
}

impl Render for PanelButtons {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let ctrl = cx.try_global::<LayoutRef>().and_then(|r| r.0.upgrade());

        let Some(ctrl) = ctrl else {
            return div();
        };

        let borrowed = ctrl.borrow();
        let entries = borrowed.panels_for_area(self.area);
        if entries.is_empty() {
            return div();
        }

        let buttons: Vec<_> = entries
            .iter()
            .map(|&(i, entry)| {
                let is_active = borrowed.is_panel_active(i);

                let dispatch = entry.dispatch;
                let mut glyph = Glyph::icon(("panel-btn", i as u64), entry.icon)
                    .label(entry.label)
                    .shortcut_by_name(entry.action_name, cx)
                    .on_click(move |window, cx| dispatch(window, cx));

                if entry.requires_active_color {
                    glyph = glyph.color(if is_active {
                        color::highlight()
                    } else {
                        color::default()
                    });
                }

                glyph.into_any_element()
            })
            .collect();

        drop(borrowed);

        // 左 dock 按钮组右侧加分隔线，右/底 dock 按钮组左侧加分隔线（与 Zed 一致）
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
