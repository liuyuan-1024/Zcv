//! PanelButtons —— 底栏按钮组。
//!
//! 每个按钮绑定一个 PanelHandle + dispatch 函数指针。

use std::sync::Arc;

use gpui::{App, Context, ElementId, Entity, Render, Window, div, prelude::*};

use crate::editor::editor::Editor;
use crate::theme::{color, space};
use crate::ui::glyph::Glyph;
use crate::workspace::dock::DockArea;
use crate::workspace::panel::PanelHandle;
use crate::workspace::status_bar::StatusItemView;

pub(crate) type PanelDispatch = fn(&mut Window, &mut App);

struct ButtonEntry {
    handle: Arc<dyn PanelHandle>,
    on_click: PanelDispatch,
}

/// 底栏按钮组。
pub(crate) struct PanelButtons {
    entries: Vec<ButtonEntry>,
}

impl PanelButtons {
    pub(crate) fn new(handles: Vec<(Arc<dyn PanelHandle>, PanelDispatch)>) -> Self {
        let entries = handles
            .into_iter()
            .map(|(handle, on_click)| ButtonEntry { handle, on_click })
            .collect();
        Self { entries }
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

        // 用第一个 entry 的位置确定分隔线方向
        let area = self
            .entries
            .first()
            .map(|e| e.handle.position())
            .unwrap_or(DockArea::Left);

        let buttons: Vec<_> = self
            .entries
            .iter()
            .map(|entry| {
                let icon_path = entry.handle.icon();
                let label = entry.handle.label();
                let action = entry.handle.action_name();
                let on_click = entry.on_click;
                Glyph::icon(ElementId::Name(icon_path.into()), icon_path)
                    .label(label)
                    .shortcut_by_name(action, cx)
                    .color(color::default())
                    .on_click(move |window, cx| on_click(window, cx))
                    .into_any_element()
            })
            .collect();

        let divider = div()
            .w(gpui::px(1.0))
            .h_full()
            .bg(color::current().gray.s[4]);

        match area {
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
