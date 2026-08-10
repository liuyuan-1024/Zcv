//! StatusBar —— 底栏容器（Entity），按 StatusItemView 模式管理状态项。
//!
//! 持有左右两侧的 StatusItemView 列表，在中心 Pane 变化时向每个 item 广播 set_active_pane_item 消息。
//! 每个 item 自行订阅 Item 变化。

use gpui::{AnyElement, App, Context, Div, Entity, Render, Subscription, Window, div, prelude::*};

use crate::ItemHandle;
use crate::pane::Pane;
use zcv_theme::{color, space};

// ═══ StatusItemView trait ═══════════════════════════════════════════

/// StatusItemView trait —— 底栏中一个可渲染的状态项。
///
/// 实现者收到 `set_active_pane_item` 通知后，应自行订阅 Item 变化并更新内部状态。
pub trait StatusItemView: Render + 'static {
    /// 当活跃标签项变化时回调。
    fn set_active_pane_item(&mut self, item: Option<&dyn ItemHandle>, cx: &mut Context<Self>);
}

/// 类型擦除桥接，让 StatusBar 存储异构 item 列表。
pub trait StatusItemViewHandle: Send {
    fn element(&self) -> AnyElement;
    fn set_active_pane_item(&self, item: Option<&dyn ItemHandle>, cx: &mut App);
}

impl<T: StatusItemView> StatusItemViewHandle for Entity<T> {
    fn element(&self) -> AnyElement {
        self.clone().into_any_element()
    }

    fn set_active_pane_item(&self, item: Option<&dyn ItemHandle>, cx: &mut App) {
        self.update(cx, |this, cx| {
            this.set_active_pane_item(item, cx);
        });
    }
}

// ═══ StatusBar Entity ═══════════════════════════════════════════════

pub struct StatusBar {
    left_items: Vec<Box<dyn StatusItemViewHandle>>,
    right_items: Vec<Box<dyn StatusItemViewHandle>>,
    pane: Entity<Pane>,
    _pane_subscription: Subscription,
}

impl StatusBar {
    // ═══ 构造与生命周期 ═══════════════════════════════════════════

    pub fn new(pane: Entity<Pane>, cx: &mut Context<Self>) -> Self {
        let pane_subscription = cx.observe(&pane, |this, pane, cx| {
            let item = pane.read(cx).active_item().map(|item| item.boxed_clone());
            for view in this.left_items.iter().chain(&this.right_items) {
                view.set_active_pane_item(item.as_deref(), cx);
            }
        });
        Self {
            left_items: Vec::new(),
            right_items: Vec::new(),
            pane,
            _pane_subscription: pane_subscription,
        }
    }

    // ═══ 注册 item ════════════════════════════════════════════════

    pub fn add_left_item<T: StatusItemView>(&mut self, item: Entity<T>, cx: &mut Context<Self>) {
        let active_item = self
            .pane
            .read(cx)
            .active_item()
            .map(|item| item.boxed_clone());
        item.set_active_pane_item(active_item.as_deref(), cx);
        self.left_items.push(Box::new(item));
        cx.notify();
    }

    pub fn add_right_item<T: StatusItemView>(&mut self, item: Entity<T>, cx: &mut Context<Self>) {
        let active_item = self
            .pane
            .read(cx)
            .active_item()
            .map(|item| item.boxed_clone());
        item.set_active_pane_item(active_item.as_deref(), cx);
        self.right_items.push(Box::new(item));
        cx.notify();
    }
}

// ═══ 渲染 ═════════════════════════════════════════════════════════

fn bar_frame(cx: &App) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .px(space::S8)
        .py(space::S6)
        .gap(space::S6)
        .bg(color::current(cx).status_bar_background)
        .text_color(color::current(cx).text)
        .border_t_1()
        .border_color(color::current(cx).border_variant)
}

fn region(items: Vec<AnyElement>, justify_start: bool) -> Div {
    let wrapper = div().flex_1().flex().items_center().gap(space::S8);
    let wrapper = if justify_start {
        wrapper.justify_start()
    } else {
        wrapper.justify_end()
    };
    wrapper.children(items)
}

impl Render for StatusBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        bar_frame(cx)
            .id("status-bar")
            .child(leading_region(&self.left_items))
            .child(trailing_region(&self.right_items))
    }
}

fn leading_region(items: &[Box<dyn StatusItemViewHandle>]) -> Div {
    let elements: Vec<AnyElement> = items.iter().map(|item| item.element()).collect();
    region(elements, true)
}

fn trailing_region(items: &[Box<dyn StatusItemViewHandle>]) -> Div {
    let elements: Vec<AnyElement> = items.iter().map(|item| item.element()).collect();
    region(elements, false)
}
