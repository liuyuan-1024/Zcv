//! Pane 顶部工具区的组合与生命周期。
//!
//! [`Toolbar`] 是 Pane 持有的持久实体；每个工具项实现 [`ToolbarItemView`]，
//! 在活动 Item 变化时返回自己的 [`ToolbarItemLocation`]。
//! 工具项由装配层注册，不经过 Item 协议，也不由具体视图返回一次性视图。

use gpui::{
    AnyView, App, Context, Entity, EntityId, EventEmitter, Render, Window, div, prelude::*,
};
use zcv_theme::{color, space};

use crate::ItemHandle;

/// 工具项依据当前活动 Item 选择的位置。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ToolbarItemLocation {
    /// 不显示。
    Hidden,
    /// 主行左侧。
    PrimaryLeft,
    /// 主行右侧。
    PrimaryRight,
    /// 主行下方的独立行。
    Secondary,
}

/// 工具项在活动 Item 不变的情况下改变自身位置时发出的事件。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ToolbarItemEvent {
    ChangeLocation(ToolbarItemLocation),
}

/// Pane 工具区的单个工具项。
///
/// 工具项是持久实体：它依据当前活动 Item 决定自身位置与显隐，而不是为某次渲染返回视图。
pub trait ToolbarItemView: Render + EventEmitter<ToolbarItemEvent> {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation;
}

trait ToolbarItemViewHandle: Send {
    fn id(&self) -> EntityId;
    fn to_any(&self) -> AnyView;
    fn set_active_pane_item(
        &self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut App,
    ) -> ToolbarItemLocation;
}

/// Pane 顶部工具区：持有全部工具项及其当前位置。
pub struct Toolbar {
    active_item: Option<Box<dyn ItemHandle>>,
    hidden: bool,
    items: Vec<(Box<dyn ToolbarItemViewHandle>, ToolbarItemLocation)>,
}

impl Toolbar {
    pub fn new() -> Self {
        Self {
            active_item: None,
            hidden: false,
            items: Vec::new(),
        }
    }

    /// 注册一个工具项，并立即按当前活动 Item 求出初始位置。
    pub fn add_item<T>(&mut self, item: Entity<T>, window: &mut Window, cx: &mut Context<Self>)
    where
        T: ToolbarItemView,
    {
        let location = item.set_active_pane_item(self.active_item.as_deref(), window, cx);
        cx.subscribe(&item, |this, item, event, cx| {
            if let Some((_, current_location)) = this
                .items
                .iter_mut()
                .find(|(candidate, _)| candidate.id() == item.entity_id())
            {
                match event {
                    ToolbarItemEvent::ChangeLocation(new_location) => {
                        if *new_location != *current_location {
                            *current_location = *new_location;
                            cx.notify();
                        }
                    }
                }
            }
        })
        .detach();
        self.items.push((Box::new(item), location));
        cx.notify();
    }

    /// 活动 Item 变化：刷新整体显隐，并让每个工具项重新选择位置。
    pub fn set_active_item(
        &mut self,
        item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_item = item.map(ItemHandle::boxed_clone);
        self.hidden = self
            .active_item
            .as_ref()
            .is_some_and(|item| !item.show_toolbar(cx));
        for (toolbar_item, current_location) in self.items.iter_mut() {
            let new_location = toolbar_item.set_active_pane_item(item, window, cx);
            if new_location != *current_location {
                *current_location = new_location;
                cx.notify();
            }
        }
    }

    fn has_any_visible_items(&self) -> bool {
        self.items
            .iter()
            .any(|(_, location)| *location != ToolbarItemLocation::Hidden)
    }

    fn items_with_location(
        &self,
        location: ToolbarItemLocation,
    ) -> impl Iterator<Item = AnyView> + '_ {
        self.items
            .iter()
            .filter(move |(_, current)| *current == location)
            .map(|(item, _)| item.to_any())
    }
}

impl Default for Toolbar {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for Toolbar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.hidden || !self.has_any_visible_items() {
            return div();
        }

        let left: Vec<AnyView> = self
            .items_with_location(ToolbarItemLocation::PrimaryLeft)
            .collect();
        let right: Vec<AnyView> = self
            .items_with_location(ToolbarItemLocation::PrimaryRight)
            .collect();
        let secondary: Vec<AnyView> = self
            .items_with_location(ToolbarItemLocation::Secondary)
            .collect();

        let colors = color::current(cx);
        let mut container = div()
            .w_full()
            .p(space::S6)
            .border_b_1()
            .border_color(colors.border)
            .flex()
            .flex_col()
            .gap(space::S6);

        let has_left = !left.is_empty();
        let has_right = !right.is_empty();
        if has_left || has_right {
            let mut row = div()
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(space::S6);
            if has_left {
                row = row.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S6)
                        .min_w_0()
                        .flex_1()
                        .children(left),
                );
            }
            if has_right {
                row = row.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S6)
                        .justify_end()
                        .when(has_left, |this| this.flex_none())
                        .when(!has_left, |this| this.flex_1())
                        .children(right),
                );
            }
            container = container.child(row);
        }

        container.children(secondary)
    }
}

impl<T: ToolbarItemView> ToolbarItemViewHandle for Entity<T> {
    fn id(&self) -> EntityId {
        self.entity_id()
    }

    fn to_any(&self) -> AnyView {
        self.clone().into()
    }

    fn set_active_pane_item(
        &self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut App,
    ) -> ToolbarItemLocation {
        self.update(cx, |this, cx| {
            this.set_active_pane_item(active_pane_item, window, cx)
        })
    }
}

#[cfg(test)]
#[path = "test/toolbar_tests.rs"]
mod tests;
