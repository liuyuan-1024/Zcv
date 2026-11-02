//! 非 macOS 的窗口内应用菜单；内容来自 GPUI 的应用菜单定义。

use gpui::{
    AnyElement, App, Context, DismissEvent, Entity, FocusHandle, Focusable, MouseDownEvent,
    OwnedMenu, OwnedMenuItem, Render, Subscription, Window, div, prelude::*,
};
use zcv_theme::{color, scale};
use zcv_ui::{Button, ContextMenu, ContextMenuEntry, PopoverHost, PopoverLayer};

use super::super::super::TopBar;

pub(in crate::top_bar) struct ApplicationMenu {
    view: Entity<ApplicationMenuView>,
    _subscription: Subscription,
}

impl ApplicationMenu {
    pub(in crate::top_bar) fn new(layer: PopoverLayer, cx: &mut Context<TopBar>) -> Self {
        let view = cx.new(|cx| ApplicationMenuView::new(layer, cx));
        let subscription = cx.observe(&view, |_, _, cx| cx.notify());
        Self {
            view,
            _subscription: subscription,
        }
    }

    pub(in crate::top_bar) fn leading_slot(&self) -> Option<AnyElement> {
        Some(self.view.clone().into_any_element())
    }

    pub(in crate::top_bar) fn is_expanded(&self, cx: &App) -> bool {
        self.view.read(cx).is_expanded()
    }
}

struct ApplicationMenuView {
    menus: Vec<OwnedMenu>,
    active_menu: usize,
    host: PopoverHost,
    /// 当前展开的菜单面板；随顶层菜单切换重建。
    menu: Option<Entity<ContextMenu>>,
    /// 打开菜单时的焦点上下文；
    /// 确认 action 前把焦点归还到这里，使 action 在打开菜单前的焦点链上派发。
    menu_context: Option<FocusHandle>,
    _menu_subscription: Option<Subscription>,
}

impl ApplicationMenuView {
    pub(super) fn new(layer: PopoverLayer, cx: &mut Context<Self>) -> Self {
        let menus = cx.get_menus().expect("应用菜单必须先于工作区安装");
        assert!(!menus.is_empty(), "应用菜单至少需要一个顶层菜单");
        for menu in &menus {
            assert!(!menu.items.is_empty(), "顶层菜单不应为空");
            assert!(
                menu.items.iter().all(|item| {
                    matches!(
                        item,
                        OwnedMenuItem::Action { .. } | OwnedMenuItem::Separator
                    )
                }),
                "窗口内应用菜单目前只支持操作与分隔线"
            );
        }
        Self {
            menus,
            active_menu: 0,
            host: PopoverHost::new(cx.focus_handle(), layer),
            menu: None,
            menu_context: None,
            _menu_subscription: None,
        }
    }

    pub(super) fn is_expanded(&self) -> bool {
        self.host.is_open()
    }

    /// 打开或切换到指定顶层菜单。
    ///
    /// 首次打开记录焦点上下文并进入浮层会话；
    /// 已展开时（悬停切换）只重建面板并转移焦点，浮层会话保持不变。
    fn open_menu(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active_menu = index;
        if !self.host.is_open() {
            self.menu_context = window.focused(cx);
        }
        let context = self.menu_context.clone();
        let menu = Self::build_panel(&self.menus[index], context, window, cx);
        let menu_focus = menu.read(cx).focus_handle(cx);
        // 先替换面板与订阅，再转移焦点：旧面板失焦时发出的关闭事件因此无人接收，不会误关整个浮层会话。
        self.menu = Some(menu.clone());
        self._menu_subscription =
            Some(
                cx.subscribe_in(&menu, window, |this, _, _: &DismissEvent, window, cx| {
                    this.dismiss_open_menu(window, cx);
                }),
            );
        if self.host.is_open() {
            self.host
                .refocus(menu_focus.clone(), menu_focus, window, cx);
        } else {
            let focus_scope = menu_focus.clone();
            self.host.toggle(focus_scope, menu_focus, window, cx);
        }
        cx.notify();
    }

    fn toggle_menu(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.host.is_open() && self.active_menu == index {
            self.dismiss_open_menu(window, cx);
        } else {
            self.open_menu(index, window, cx);
        }
    }

    /// 鼠标悬停切换顶层菜单；仅在已展开时生效。
    fn hover_menu(
        &mut self,
        index: usize,
        entered: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if entered && self.host.is_open() && self.active_menu != index {
            self.open_menu(index, window, cx);
        }
    }

    fn dismiss_open_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        self.menu_context = None;
        self._menu_subscription = None;
        self.host.dismiss(window, cx);
        cx.notify();
    }

    /// 点击面板外时关闭；若点击的是相邻菜单的触发器，悬停切换已接管会话，不在此关闭。
    fn close_on_outside_click(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active_menu = self.active_menu;
        let menu = cx.weak_entity();
        window.defer(cx, move |window, cx| {
            let _ = menu.update(cx, |menu, cx| {
                if menu.active_menu == active_menu && menu.host.is_open() {
                    menu.dismiss_open_menu(window, cx);
                }
            });
        });
    }

    /// 把 GPUI 应用菜单定义转换成菜单面板；快捷键文本来自 keymap。
    fn build_panel(
        menu: &OwnedMenu,
        context: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<ContextMenu> {
        ContextMenu::build(window, cx, |panel, _, cx| {
            let panel = match context {
                Some(context) => panel.context(context),
                None => panel,
            };
            menu.items.iter().fold(panel, |panel, item| match item {
                OwnedMenuItem::Separator => panel.separator(),
                OwnedMenuItem::Action {
                    name,
                    action,
                    checked,
                    disabled,
                    ..
                } => {
                    let mut entry = ContextMenuEntry::action(name.clone(), action.boxed_clone())
                        .checked(*checked)
                        .disabled(*disabled);
                    if let Some(shortcut) = zcv_keymap::display_shortcut(&**action, cx) {
                        entry = entry.end_slot_text(shortcut);
                    }
                    panel.entry(entry)
                }
                _ => unreachable!("菜单内容已经在构造时校验"),
            })
        })
    }
}

#[cfg(test)]
#[path = "../../../test/application_menu_tests.rs"]
mod tests;

impl Render for ApplicationMenuView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let expanded = self.host.is_open();
        let mut root = div()
            .track_focus(&self.host.focus_handle())
            .flex()
            .items_center()
            .gap(scale::S2);

        for (menu_index, menu) in self.menus.iter().enumerate() {
            if !expanded && menu_index != 0 {
                break;
            }
            let trigger = if expanded {
                Button::text(
                    ("application-menu-trigger", menu_index),
                    menu.name.to_string(),
                )
            } else {
                Button::icon(("application-menu-trigger", menu_index), "icons/menu.svg")
                    .label("应用菜单")
            };
            let trigger = trigger.no_occlude().disabled(menu.disabled).on_click(
                cx.listener(move |this, _, window, cx| this.toggle_menu(menu_index, window, cx)),
            );
            let mut slot = div()
                .id(("application-menu", menu_index))
                .relative()
                .rounded_sm()
                .child(trigger)
                .on_hover(cx.listener(move |this, entered, window, cx| {
                    this.hover_menu(menu_index, *entered, window, cx)
                }));
            if expanded && self.active_menu == menu_index {
                slot = slot.bg(color::current(cx).element_selected);
                if let Some(menu) = self.menu.clone() {
                    let on_outside = cx.listener(Self::close_on_outside_click);
                    slot = slot.child(self.host.menu_overlay(window, menu, on_outside));
                }
            }
            root = root.child(slot);
        }
        root
    }
}
