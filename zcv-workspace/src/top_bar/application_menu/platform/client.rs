//! 非 macOS 的窗口内应用菜单；内容来自 GPUI 的应用菜单定义。

use gpui::{
    AnyElement, App, Context, Entity, MouseDownEvent, OwnedMenu, OwnedMenuItem, Render,
    StatefulInteractiveElement, Subscription, Window, div, prelude::*, px,
};
use zcv_actions::CloseMenu;
use zcv_theme::{color, scale};
use zcv_ui::{Button, ListItem, PopoverHost, PopoverLayer, SvgIcon};

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
        }
    }

    pub(super) fn is_expanded(&self) -> bool {
        self.host.is_open()
    }

    fn open_menu(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active_menu = index;
        if !self.host.is_open() {
            let focus = self.host.focus_handle();
            self.host.toggle(focus.clone(), focus, window, cx);
        }
        cx.notify();
    }

    fn toggle_menu(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.host.is_open() && self.active_menu == index {
            self.host.dismiss(window, cx);
            cx.notify();
        } else {
            self.open_menu(index, window, cx);
        }
    }

    fn hover_menu(&mut self, index: usize, entered: bool, cx: &mut Context<Self>) {
        if entered && self.host.is_open() && self.active_menu != index {
            self.active_menu = index;
            cx.notify();
        }
    }

    fn close(&mut self, _: &CloseMenu, window: &mut Window, cx: &mut Context<Self>) {
        self.host.dismiss(window, cx);
        cx.notify();
    }

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
                    menu.host.dismiss(window, cx);
                    cx.notify();
                }
            });
        });
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.host.is_open() {
            return;
        }
        let Some(OwnedMenuItem::Action {
            action,
            disabled: false,
            ..
        }) = self.menus[self.active_menu].items.get(index)
        else {
            return;
        };
        let action = action.boxed_clone();
        self.host.dismiss(window, cx);
        window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
        cx.notify();
    }

    /// 单个菜单的面板内容：名称靠左、快捷键靠行尾。
    ///
    /// 面板宽度由内容撑开；勾选列只在菜单含勾选项时占用，普通菜单项不预留缩进。
    fn render_panel(&self, menu: &OwnedMenu, window: &Window, cx: &Context<Self>) -> AnyElement {
        let colors = *color::current(cx);
        let has_toggles = menu
            .items
            .iter()
            .any(|item| matches!(item, OwnedMenuItem::Action { checked: true, .. }));
        let mut rows = div()
            .id("application-menu-rows")
            // test cfg 下注册 debug bounds，供面板宽度断言使用。
            .debug_selector(|| "application-menu-rows".into())
            .max_h(window.bounds().size.height - scale::to_pixels(scale::S12, window))
            .py(scale::S4)
            .overflow_y_scroll();

        for (item_index, item) in menu.items.iter().enumerate() {
            match item {
                OwnedMenuItem::Separator => {
                    rows = rows.child(
                        div()
                            .h(px(1.0))
                            .my(scale::S4)
                            .mx(scale::S8)
                            .bg(colors.border),
                    );
                }
                OwnedMenuItem::Action {
                    name,
                    action,
                    checked,
                    disabled,
                    ..
                } => {
                    let checked = *checked;
                    let disabled = *disabled;
                    let label_color = if disabled {
                        colors.text_disabled
                    } else {
                        colors.text
                    };
                    let shortcut_color = if disabled {
                        colors.text_disabled
                    } else {
                        colors.text_placeholder
                    };
                    let row = ListItem::new(("application-menu-item", item_index))
                        .disabled(disabled)
                        .child(
                            div()
                                .debug_selector(move || {
                                    format!("application-menu-label-{item_index}")
                                })
                                .text_color(label_color)
                                .child(name.clone()),
                        )
                        // 快捷键与顶栏按钮同一数据源；没有绑定的菜单项不留占位。
                        .when_some(
                            zcv_keymap::display_shortcut(&**action, cx),
                            move |row, shortcut| {
                                row.end_slot(
                                    div()
                                        .debug_selector(move || {
                                            format!("application-menu-shortcut-{item_index}")
                                        })
                                        .text_color(shortcut_color)
                                        .child(shortcut),
                                )
                            },
                        )
                        .when(has_toggles, move |row| {
                            row.start_slot(
                                div()
                                    .debug_selector(move || {
                                        format!("application-menu-check-{item_index}")
                                    })
                                    // 未勾选的项用不可见占位保持同列对齐。
                                    .when(!checked, |slot| slot.invisible())
                                    .child(SvgIcon::new("icons/check.svg").color(if disabled {
                                        colors.text_disabled
                                    } else {
                                        colors.icon_accent
                                    })),
                            )
                        });
                    rows = rows.child(
                        div()
                            .id(("application-menu-row", item_index))
                            .debug_selector(move || format!("application-menu-item-{item_index}"))
                            .when(!disabled, |row| {
                                row.on_click(cx.listener(move |this, _, window, cx| {
                                    this.activate(item_index, window, cx)
                                }))
                            })
                            .child(row),
                    );
                }
                _ => unreachable!("菜单内容已经在构造时校验"),
            }
        }

        rows.into_any_element()
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
            .key_context("ApplicationMenu")
            .on_action(cx.listener(Self::close))
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
                .on_hover(cx.listener(move |this, entered, _, cx| {
                    this.hover_menu(menu_index, *entered, cx)
                }));
            if expanded && self.active_menu == menu_index {
                slot = slot.bg(color::current(cx).element_selected);
                let panel = self.render_panel(menu, window, cx);
                slot = slot.child(self.host.menu_overlay(
                    window,
                    cx,
                    panel,
                    cx.listener(Self::close_on_outside_click),
                ));
            }
            root = root.child(slot);
        }
        root
    }
}
