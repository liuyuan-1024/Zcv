use gpui::{
    App, Context, DismissEvent, ElementId, Entity, Focusable, SharedString, Subscription, Window,
    div, prelude::*,
};
use zcv_theme::{color, scale};

use crate::{
    ButtonLike, ButtonStyle, ContextMenu, ContextMenuEntry, OverlayAnchor, OverlayOutside,
    PopoverHost, PopoverLayer, SvgIcon, overlay,
};

/// 显示当前值并打开锚定菜单。每次打开使用调用方当前提供的条目。
#[derive(IntoElement)]
pub struct DropdownMenu {
    id: ElementId,
    value: SharedString,
    entries: Vec<ContextMenuEntry>,
    layer: PopoverLayer,
}

impl DropdownMenu {
    pub fn new(
        id: impl Into<ElementId>,
        value: impl Into<SharedString>,
        entries: Vec<ContextMenuEntry>,
        layer: PopoverLayer,
    ) -> Self {
        Self {
            id: id.into(),
            value: value.into(),
            entries,
            layer,
        }
    }
}

struct OpenMenu {
    menu: Entity<ContextMenu>,
    _dismiss: Subscription,
}

struct DropdownSession {
    host: PopoverHost,
    open_menu: Option<OpenMenu>,
}

impl DropdownSession {
    fn open(&mut self, entries: &[ContextMenuEntry], window: &mut Window, cx: &mut Context<Self>) {
        let menu = ContextMenu::build(window, cx, |menu, _, _| {
            entries
                .iter()
                .cloned()
                .fold(menu, ContextMenu::entry)
                .select_checked_or_first()
        });
        let dismiss = cx.subscribe_in(&menu, window, |session, _, _: &DismissEvent, window, cx| {
            session.close(window, cx);
        });
        let focus = menu.focus_handle(cx);
        self.open_menu = Some(OpenMenu {
            menu,
            _dismiss: dismiss,
        });
        window.focus(&self.host.focus_handle(), cx);
        self.host.toggle(focus.clone(), focus, window, cx);
        cx.notify();
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.host.dismiss(window, cx);
        self.open_menu.take();
        cx.notify();
    }
}

impl Drop for DropdownSession {
    fn drop(&mut self) {
        self.host.clear();
    }
}

impl RenderOnce for DropdownMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let session = window.with_id(self.id.clone(), |window| {
            window.use_keyed_state("dropdown-session", cx, |_, cx| DropdownSession {
                host: PopoverHost::new(cx.focus_handle().tab_stop(true), self.layer),
                open_menu: None,
            })
        });
        let state = session.read(cx);
        let focus = state.host.focus_handle();
        let menu = state
            .open_menu
            .as_ref()
            .filter(|_| state.host.is_open())
            .map(|open| open.menu.clone());
        let colors = *color::current(cx);
        let click = session.downgrade();
        let trigger = ButtonLike::new("trigger")
            .style(ButtonStyle::Solid)
            .padding(scale::S6)
            .on_click(move |_, window, cx| {
                click
                    .update(cx, |state, cx| {
                        if state.host.is_open() {
                            state.close(window, cx);
                        } else {
                            state.open(&self.entries, window, cx);
                        }
                    })
                    .ok();
            })
            .child(
                div()
                    .flex()
                    .min_w(scale::structural(104.0))
                    .items_center()
                    .justify_between()
                    .gap(scale::S8)
                    .child(div().text_color(colors.text).child(self.value))
                    .child(
                        SvgIcon::new("icons/chevron_up_down.svg")
                            .size(scale::to_pixels(scale::S12, window))
                            .color(colors.icon_muted),
                    ),
            )
            .render_root(cx)
            .debug_selector(|| "dropdown-trigger".into())
            .track_focus(&focus)
            .focus_visible(move |style| style.border_color(colors.border_focused));

        div()
            .id(self.id)
            .relative()
            .flex()
            .self_start()
            .child(trigger)
            .when_some(menu, |root, menu| {
                let state = session.downgrade();
                root.child(
                    overlay(window)
                        .anchor(OverlayAnchor::AtTrigger)
                        .outside(OverlayOutside::Intercept(Box::new(move |_, window, cx| {
                            state.update(cx, |state, cx| state.close(window, cx)).ok();
                        })))
                        .child(menu),
                )
            })
    }
}

#[cfg(test)]
#[path = "test/dropdown_menu_tests.rs"]
mod tests;
