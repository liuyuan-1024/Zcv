//! 非 macOS 的窗口内应用菜单；内容来自 GPUI 的应用菜单定义。

use gpui::{Action, Context, OwnedMenuItem, Render, SharedString, Window, div, prelude::*, px};
use zcv_actions::{CloseMenu, ConfirmItem, NextItem, PreviousItem};
use zcv_keymap::display_shortcut;
use zcv_theme::color;
use zcv_ui::{Button, ListItem, PopoverHost, PopoverLayer};

struct MenuAction {
    name: SharedString,
    action: Box<dyn Action>,
}

pub(super) struct ApplicationMenu {
    name: SharedString,
    items: Vec<MenuAction>,
    selected: usize,
    host: PopoverHost,
}

impl ApplicationMenu {
    pub(super) fn new(layer: PopoverLayer, cx: &mut Context<Self>) -> Self {
        let mut menus = cx.get_menus().expect("应用菜单必须先于工作区安装");
        assert_eq!(menus.len(), 1, "当前顶栏只呈现一个应用菜单");
        let menu = menus.remove(0);
        let items = menu
            .items
            .into_iter()
            .map(|item| match item {
                OwnedMenuItem::Action { name, action, .. } => MenuAction {
                    name: name.into(),
                    action,
                },
                _ => panic!("当前应用菜单只支持 Action 条目"),
            })
            .collect();
        Self {
            name: menu.name.into(),
            items,
            selected: 0,
            host: PopoverHost::new(cx.focus_handle(), layer),
        }
    }

    fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = 0;
        let focus = self.host.focus_handle();
        self.host.toggle(focus.clone(), focus, window, cx);
        cx.notify();
    }

    fn close(&mut self, _: &CloseMenu, window: &mut Window, cx: &mut Context<Self>) {
        self.host.dismiss(window, cx);
        cx.notify();
    }

    fn select_next(&mut self, _: &NextItem, _: &mut Window, cx: &mut Context<Self>) {
        if self.host.is_open() && !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
            cx.notify();
        }
    }

    fn select_previous(&mut self, _: &PreviousItem, _: &mut Window, cx: &mut Context<Self>) {
        if self.host.is_open() && !self.items.is_empty() {
            self.selected = (self.selected + self.items.len() - 1) % self.items.len();
            cx.notify();
        }
    }

    fn confirm(&mut self, _: &ConfirmItem, window: &mut Window, cx: &mut Context<Self>) {
        self.activate(self.selected, window, cx);
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(index) else {
            return;
        };
        let action = item.action.boxed_clone();
        self.host.dismiss(window, cx);
        window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
        cx.notify();
    }
}

#[cfg(test)]
#[path = "../test/application_menu_tests.rs"]
mod tests;

impl Render for ApplicationMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .track_focus(&self.host.focus_handle())
            .key_context("ApplicationMenu")
            .on_action(cx.listener(Self::close))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::confirm))
            .relative()
            .child(
                Button::text("application-menu", self.name.to_string())
                    .label("应用菜单")
                    .on_click(cx.listener(|menu, _, window, cx| menu.toggle(window, cx))),
            );
        if self.host.is_open() {
            let rows = self.items.iter().enumerate().map(|(index, item)| {
                let shortcut = display_shortcut(item.action.as_ref(), cx);
                let mut row = ListItem::new(index)
                    .toggle_state(index == self.selected)
                    .child(item.name.clone());
                if let Some(shortcut) = shortcut {
                    row = row.end_slot(
                        div()
                            .text_color(color::current(cx).text_placeholder)
                            .child(shortcut),
                    );
                }
                div()
                    .id(("application-menu-item", index))
                    .child(row)
                    .on_click(
                        cx.listener(move |menu, _, window, cx| menu.activate(index, window, cx)),
                    )
            });
            root = root.child(
                self.host
                    .overlay(window, cx, div().w(px(220.0)).children(rows)),
            );
        }
        root
    }
}
