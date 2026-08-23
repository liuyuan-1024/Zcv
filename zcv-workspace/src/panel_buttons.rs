//! PanelButtons —— 底栏按钮组。
//!
//! 每个 DockPosition 一个按钮组，持有对应的 Dock Entity 来查询面板激活状态，激活时高亮显示。
//! 参考 Zed `crates/workspace/src/dock.rs`。

use gpui::{
    App, ClickEvent, Context, ElementId, Entity, Render, Subscription, WeakEntity, Window, div,
    prelude::*,
};
use zcv_theme::{color, space};
use zcv_ui::Glyph;

use crate::{FocusOrHidePanel, ItemHandle, Workspace};
use crate::{
    dock::{Dock, DockPosition},
    status_bar::StatusItemView,
};

/// 底栏按钮组：绑定一个 Dock Entity，渲染其所有面板。
pub struct PanelButtons {
    dock: Entity<Dock>,
    workspace: WeakEntity<Workspace>,
    _subscription: Subscription,
}

impl PanelButtons {
    pub fn new(
        dock: Entity<Dock>,
        workspace: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Dock 状态变化时自动重绘
        let sub = cx.observe(&dock, |_, _, cx| cx.notify());
        Self {
            dock,
            workspace,
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
        if dock.panel_count() == 0 {
            return div();
        }

        let active_index = dock.active_panel_index();
        let is_open = dock.is_open();
        let area = dock.position();

        let buttons: Vec<_> = dock
            .panels()
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
                // tooltip 始终描述 Panel 的键盘命令。
                let shortcut_action = FocusOrHidePanel::new(handle.persistent_name());
                let on_click = {
                    let workspace = self.workspace.clone();
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        workspace
                            .update(cx, |workspace, cx| {
                                workspace.toggle_panel_visibility_from_button(area, i, window, cx);
                            })
                            .ok();
                    }
                };

                Glyph::icon(ElementId::Name(icon_path.into()), icon_path)
                    .label(label)
                    .shortcut(&shortcut_action, cx)
                    .color(fg)
                    .on_click(on_click)
                    .into_any_element()
            })
            .collect();

        // 分隔线由按钮组自己绘制，跟随自身可见性，不存在悬空线：
        // 左侧面板按钮组的分隔线画在按钮组之后；
        // 右侧面板按钮组的分隔线画在按钮组之前，承担与底栏按钮组之间的分隔；
        // 底栏按钮组不画线——它在右侧区域中部，画前导线时一旦左侧状态项隐藏（如未打开文件）就会悬空。
        let divider = div()
            .w(gpui::px(1.0))
            .h_full()
            .bg(color::current(cx).border);
        let content = div().flex().items_center().gap(space::S6).children(buttons);
        match area {
            DockPosition::Left => div()
                .flex()
                .items_center()
                .gap(space::S8)
                .child(content)
                .child(divider),
            DockPosition::Right => div()
                .flex()
                .items_center()
                .gap(space::S8)
                .child(divider)
                .child(content),
            DockPosition::Bottom => content,
        }
    }
}
