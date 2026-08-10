//! Toolbar —— Pane 内容区顶部的工具条。
//!
//! 左侧承载面包屑，右侧承载活动文件相关操作。

use gpui::{
    AnyView, App, Context, Entity, EntityId, EventEmitter, Render, Window, div, prelude::*,
};

use super::{ToggleFileSearch, TogglePreview};
use zcv_theme::{color, space};
use zcv_ui::Glyph;
use zcv_workspace::{ItemHandle, ItemPresentation, ToolbarItemLocation};

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

// ═══ 活动文件右侧控件 ════════════════════════════════════════════

pub(crate) struct FileToolbarControls {
    visible: bool,
    presentation: Option<ItemPresentation>,
}

impl FileToolbarControls {
    pub(crate) fn new() -> Self {
        Self {
            visible: false,
            presentation: None,
        }
    }
}

impl EventEmitter<ToolbarItemEvent> for FileToolbarControls {}

impl ToolbarItemView for FileToolbarControls {
    fn set_active_item(
        &mut self,
        active_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        let path = active_item.and_then(|item| item.file_path(cx));
        self.visible = path.is_some();
        self.presentation = active_item
            .and_then(|item| item.document_item_key(cx))
            .map(|key| key.presentation)
            .filter(|presentation| {
                matches!(presentation, ItemPresentation::Preview(_))
                    || path
                        .as_deref()
                        .is_some_and(|path| zcv_preview::provider_for(path, cx).is_some())
            });
        cx.notify();
        if self.visible {
            ToolbarItemLocation::PrimaryRight
        } else {
            ToolbarItemLocation::Hidden
        }
    }
}

impl Render for FileToolbarControls {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap(space::S6)
            .when_some(self.presentation, |controls, presentation| {
                let (icon, label, icon_color) = match presentation {
                    ItemPresentation::Preview(_) => (
                        "icons/eye_off.svg",
                        "源码".to_string(),
                        color::current(cx).text_muted,
                    ),
                    ItemPresentation::Source => (
                        "icons/eye.svg",
                        "预览".to_string(),
                        color::current(cx).text_muted,
                    ),
                };
                controls.child(
                    Glyph::icon("toolbar-preview", icon)
                        .label(label)
                        .color(icon_color)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(TogglePreview), cx)
                        }),
                )
            })
            .child(
                Glyph::icon("toolbar-file-search", "icons/magnifying_glass.svg")
                    .label("搜索")
                    .shortcut(&ToggleFileSearch, cx)
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(ToggleFileSearch), cx)
                    }),
            )
    }
}

// ═══ Render ═════════════════════════════════════════════════════════

impl Render for Toolbar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut left_elements: Vec<AnyView> = Vec::new();
        let mut right_elements: Vec<AnyView> = Vec::new();

        for (item, location) in &self.items {
            match location {
                ToolbarItemLocation::Hidden => {}
                ToolbarItemLocation::PrimaryLeft => left_elements.push(item.to_any()),
                ToolbarItemLocation::PrimaryRight => right_elements.push(item.to_any()),
            }
        }

        if left_elements.is_empty() && right_elements.is_empty() {
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
            .border_color(color::current(cx).border_variant)
            .bg(color::current(cx).toolbar_background)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S6)
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .justify_start()
                            .overflow_x_hidden()
                            .children(left_elements),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_end()
                            .children(right_elements),
                    ),
            )
    }
}
