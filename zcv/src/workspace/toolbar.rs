//! Toolbar —— Pane 内容区顶部的工具条。
//!
//! 包含面包屑导航、搜索栏、诊断控件等，按 PrimaryLeft / PrimaryRight / Secondary 布局。

use gpui::{
    AnyView, App, Context, Entity, EntityId, EventEmitter, Render, Window, div, prelude::*,
};

use super::item::ItemHandle;
use crate::theme::{color, space};

// ═══ ToolbarItemLocation ═════════════════════════════════════════════

/// Toolbar 子项的布局位置。
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum ToolbarItemLocation {
    /// 不显示。
    Hidden,
    /// 左区（面包屑等）。
    PrimaryLeft,
    /// 右区（搜索栏、控件按钮等）。
    PrimaryRight,
    /// 底部整行（横幅提示等）。
    Secondary,
}

// ═══ ToolbarItemEvent ════════════════════════════════════════════════

/// Toolbar 子项向 Toolbar 发出的事件。
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) enum ToolbarItemEvent {
    /// 子项要求变更显示位置。
    ChangeLocation(ToolbarItemLocation),
}

// ═══ ToolbarItemView trait ═══════════════════════════════════════════

/// Toolbar 子项需实现的接口。
pub(crate) trait ToolbarItemView: Render + EventEmitter<ToolbarItemEvent> {
    /// 当前激活的 item 切换时调用，返回此子项应显示的位置。
    fn set_active_item(
        &mut self,
        active_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation;
}

// ═══ ToolbarItemViewHandle trait object ═══════════════════════════════

/// 抹消具体类型的 Toolbar 子项句柄。
pub(crate) trait ToolbarItemViewHandle: Send {
    fn id(&self) -> EntityId;
    fn to_any(&self) -> AnyView;
    fn set_active_item(
        &self,
        active_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut App,
    ) -> ToolbarItemLocation;
}

/// 桥接：任何 `Entity<T: ToolbarItemView>` 自动实现 `ToolbarItemViewHandle`。
impl<T: ToolbarItemView + 'static> ToolbarItemViewHandle for Entity<T> {
    fn id(&self) -> EntityId {
        self.entity_id()
    }

    fn to_any(&self) -> AnyView {
        self.clone().into()
    }

    fn set_active_item(
        &self,
        active_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut App,
    ) -> ToolbarItemLocation {
        self.update(cx, |this, cx| this.set_active_item(active_item, window, cx))
    }
}

// ═══ Toolbar struct ══════════════════════════════════════════════════

/// Pane 内容区顶部的工具条 Entity。
pub(crate) struct Toolbar {
    active_item: Option<Box<dyn ItemHandle>>,
    items: Vec<(Box<dyn ToolbarItemViewHandle>, ToolbarItemLocation)>,
}

impl Default for Toolbar {
    fn default() -> Self {
        Self::new()
    }
}

impl Toolbar {
    pub fn new() -> Self {
        Self {
            active_item: None,
            items: Vec::new(),
        }
    }

    /// 注册一个子项。
    ///
    /// 注册时立即传入当前 item 让子项确定初始位置，同时订阅子项的
    /// `ChangeLocation` 事件以便位置变更时重绘。
    pub fn add_item<T>(&mut self, item: Entity<T>, window: &mut Window, cx: &mut Context<Self>)
    where
        T: 'static + ToolbarItemView,
    {
        let location = item.set_active_item(self.active_item.as_deref(), window, cx);
        cx.subscribe(&item, |this, item, event, cx| {
            if let Some((_, current_location)) = this
                .items
                .iter_mut()
                .find(|(i, _)| i.id() == item.entity_id())
            {
                match event {
                    ToolbarItemEvent::ChangeLocation(new_location) => {
                        if new_location != current_location {
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

    /// 设置当前激活的 item（Pane 切换 tab 时调用）。
    pub fn set_active_item(
        &mut self,
        item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_item = item.map(|item| item.boxed_clone());

        for (toolbar_item, current_location) in self.items.iter_mut() {
            let new_location = toolbar_item.set_active_item(item, window, cx);
            if new_location != *current_location {
                *current_location = new_location;
                cx.notify();
            }
        }
    }
}

// ═══ Render ═════════════════════════════════════════════════════════

impl Render for Toolbar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // 按位置分组（用显式循环避免复杂迭代器链的类型推断问题）
        let mut left_elements: Vec<AnyView> = Vec::new();
        let mut right_elements: Vec<AnyView> = Vec::new();
        let mut secondary_elements: Vec<AnyView> = Vec::new();

        for (item, location) in &self.items {
            match location {
                ToolbarItemLocation::Hidden => {}
                ToolbarItemLocation::PrimaryLeft => left_elements.push(item.to_any()),
                ToolbarItemLocation::PrimaryRight => right_elements.push(item.to_any()),
                ToolbarItemLocation::Secondary => secondary_elements.push(item.to_any()),
            }
        }

        let has_left = !left_elements.is_empty();
        let has_right = !right_elements.is_empty();

        if !has_left && !has_right && secondary_elements.is_empty() {
            return div();
        }

        div()
            .group("toolbar")
            .relative()
            .flex()
            .flex_col()
            .py(space::S6)
            .px(space::S8)
            .border_b_1()
            .border_color(color::current().gray.s[4])
            .bg(color::current().gray.s[1])
            .when(has_left || has_right, |this| {
                this.child(
                    div()
                        .flex()
                        .items_start()
                        .justify_between()
                        .gap(space::S6)
                        .when(has_left, |this| {
                            this.child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .justify_start()
                                    .overflow_x_hidden()
                                    .children(left_elements),
                            )
                        })
                        .when(has_right, |this| {
                            this.child(
                                div()
                                    .flex()
                                    .flex_row_reverse()
                                    .when(has_left, |this| this.flex_none())
                                    .justify_end()
                                    .children(right_elements),
                            )
                        }),
                )
            })
            .children(secondary_elements)
    }
}
