//! ContextMenu —— 独立菜单组件。
//!
//! 按组分批展示，组间自动插分割线。
//! 内部自己管理选中项和键盘导航。
//!
//! # 用法
//!
//! ```ignore
//! let groups = ContextMenu::build()
//!     .group(|g| g
//!         .action_item("打开文件", OpenFile)
//!         .action_item("打开项目", ToggleProjectTree)
//!     )
//!     .group(|g| g
//!         .action_item("设置", OpenSettings)
//!     )
//!     .finish();
//!
//! let menu = cx.new(|cx| ContextMenu::new(groups, cx));
//! ```

use std::cell::Cell;
use std::rc::Rc;

use gpui::{AnyElement, App, Context, FocusHandle, Render, Window, div, prelude::*, px};

use crate::theme::{color, radius, space, typography};

// ═══ 数据模型 ═════════════════════════════════════════════════════════

pub(crate) struct ContextMenuGroup {
    items: Vec<ContextMenuItem>,
}

struct ContextMenuItem {
    label: &'static str,
    /// 快捷键文字显示（例如 "⌘O"）。None 则不显示。
    shortcut: Option<&'static str>,
    handler: Rc<dyn Fn(&mut Window, &mut App)>,
}

// ═══ Builder ══════════════════════════════════════════════════════════

impl ContextMenu {
    /// 创建 ContextMenu Builder。
    pub(crate) fn build() -> ContextMenuBuilder {
        ContextMenuBuilder::new()
    }
}

/// ContextMenu 构建器。
pub(crate) struct ContextMenuBuilder {
    groups: Vec<Vec<ContextMenuItem>>,
}

impl ContextMenuBuilder {
    fn new() -> Self {
        Self { groups: Vec::new() }
    }

    /// 添加一个功能组。组间在渲染时会自动插入分割线。
    pub(crate) fn group(
        mut self,
        f: impl FnOnce(ContextMenuGroupBuilder) -> ContextMenuGroupBuilder,
    ) -> Self {
        let builder = ContextMenuGroupBuilder::new();
        let built = f(builder);
        self.groups.push(built.items);
        self
    }

    /// 完成构建，返回 groups 数据。
    pub(crate) fn finish(self) -> Vec<ContextMenuGroup> {
        self.groups
            .into_iter()
            .map(|items| ContextMenuGroup { items })
            .collect()
    }
}

/// 单组构建器。
pub(crate) struct ContextMenuGroupBuilder {
    items: Vec<ContextMenuItem>,
}

impl ContextMenuGroupBuilder {
    fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// 添加快捷键和点击都走 action 的菜单项。
    ///
    /// 快捷键文字自动从 action 名称查 KeyBindings 获得。
    pub(crate) fn action_item<A: gpui::Action + Clone>(
        mut self,
        label: &'static str,
        action: A,
    ) -> Self {
        self.items.push(ContextMenuItem {
            label,
            shortcut: None, // 快捷键从 KeyBindings 查，由调用方在 surface 层处理
            handler: Rc::new(move |window, cx| {
                window.dispatch_action(Box::new(action.clone()), cx);
            }),
        });
        self
    }

    /// 添加自定义菜单项（手动指定快捷键文字和回调）。
    pub(crate) fn item(
        mut self,
        label: &'static str,
        shortcut: Option<&'static str>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(ContextMenuItem {
            label,
            shortcut,
            handler: Rc::new(handler),
        });
        self
    }
}

// ═══ Entity ══════════════════════════════════════════════════════════

/// ContextMenu Entity。
///
/// 内部维护选中项索引和焦点句柄，在 render() 中处理键盘导航。
pub(crate) struct ContextMenu {
    groups: Vec<ContextMenuGroup>,
    /// 扁平化后的选中索引（跨越所有组）。
    selected_index: Rc<Cell<usize>>,
    focus: FocusHandle,
}

impl ContextMenu {
    pub(crate) fn new(groups: Vec<ContextMenuGroup>, cx: &mut Context<Self>) -> Self {
        Self {
            groups,
            selected_index: Rc::new(Cell::new(0)),
            focus: cx.focus_handle(),
        }
    }

    fn item_count(&self) -> usize {
        self.groups.iter().map(|g| g.items.len()).sum()
    }

    /// 获取当前选中的 item（如果有）。
    fn current_item(&self) -> Option<&ContextMenuItem> {
        let idx = self.selected_index.get();
        let mut flat = 0usize;
        for group in &self.groups {
            if idx < flat + group.items.len() {
                return Some(&group.items[idx - flat]);
            }
            flat += group.items.len();
        }
        None
    }

    /// 纯静态渲染，不含键盘导航。用于嵌入其他组件（如 Picker）。
    pub(crate) fn render_inline(&self) -> AnyElement {
        render_menu(&self.groups, None)
    }

    fn render_item(&self, item: &ContextMenuItem, is_selected: bool) -> AnyElement {
        let handler = item.handler.clone();

        div()
            .flex()
            .flex_row()
            .items_center()
            .px(space::S6)
            .py(space::S2)
            .gap(space::S8)
            .text_color(color::current().gray.s[8])
            .cursor_pointer()
            .hover(|style| {
                if !is_selected {
                    style.bg(color::current().gray.s[3])
                } else {
                    style
                }
            })
            .bg(if is_selected {
                color::current().gray.s[3]
            } else {
                gpui::rgba(0)
            })
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                handler(window, cx);
            })
            .child(div().flex_1().text_size(typography::ui()).child(item.label))
            .children(item.shortcut.map(|s| {
                div()
                    .text_color(color::current().gray.s[5])
                    .text_size(typography::ui())
                    .child(s)
                    .into_any_element()
            }))
            .into_any_element()
    }
}

/// 静态渲染：将 groups 渲染为带分割线的菜单元素。
/// 如果 selected 为 None，不做选中高亮（用于 render_inline）。
fn render_menu(groups: &[ContextMenuGroup], selected: Option<usize>) -> AnyElement {
    if groups.is_empty() || groups.iter().all(|g| g.items.is_empty()) {
        return div().into_any_element();
    }

    let mut children: Vec<AnyElement> = Vec::new();
    let mut flat_idx = 0usize;

    for (gi, group) in groups.iter().enumerate() {
        if gi > 0 {
            children.push(divider());
        }

        for item in &group.items {
            let is_selected = selected.map_or(false, |s| s == flat_idx);
            children.push(render_item_element(item, is_selected));
            flat_idx += 1;
        }
    }

    div()
        .flex()
        .flex_col()
        .min_w(px(180.0))
        .bg(color::current().gray.s[2])
        .border_1()
        .border_color(color::current().gray.s[4])
        .rounded(radius::R4)
        .overflow_hidden()
        .py(space::S4)
        .children(children)
        .into_any_element()
}

/// 渲染单个菜单项元素。
fn render_item_element(item: &ContextMenuItem, is_selected: bool) -> AnyElement {
    let handler = item.handler.clone();

    div()
        .flex()
        .flex_row()
        .items_center()
        .px(space::S6)
        .py(space::S2)
        .gap(space::S8)
        .text_color(color::current().gray.s[8])
        .cursor_pointer()
        .hover(|style| {
            if !is_selected {
                style.bg(color::current().gray.s[3])
            } else {
                style
            }
        })
        .bg(if is_selected {
            color::current().gray.s[3]
        } else {
            gpui::rgba(0)
        })
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
            handler(window, cx);
        })
        .child(div().flex_1().text_size(typography::ui()).child(item.label))
        .children(item.shortcut.map(|s| {
            div()
                .text_color(color::current().gray.s[5])
                .text_size(typography::ui())
                .child(s)
                .into_any_element()
        }))
        .into_any_element()
}

/// 组间分割线。
fn divider() -> AnyElement {
    div()
        .w_full()
        .h(px(1.0))
        .bg(color::current().gray.s[4])
        .into_any_element()
}

// ═══ 键盘导航 ═══════════════════════════════════════════════════════

impl Render for ContextMenu {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let total = self.item_count();
        let selected = self.selected_index.clone();

        let handle_key = move |event: &gpui::KeyDownEvent, window: &mut Window, _cx: &mut App| {
            match event.keystroke.key.as_str() {
                "up" if total > 0 => {
                    let mut idx = selected.get();
                    idx = if idx == 0 { total - 1 } else { idx - 1 };
                    selected.set(idx);
                    window.refresh();
                }
                "down" if total > 0 => {
                    let mut idx = selected.get();
                    idx = if idx >= total - 1 { 0 } else { idx + 1 };
                    selected.set(idx);
                    window.refresh();
                }
                "enter" if total > 0 => {
                    // 执行选中项 — 由外层 Surface 处理
                    // 当前只留占位，后续接入 Surface 时实现
                }
                _ => {}
            }
        };

        let mut children: Vec<AnyElement> = Vec::new();
        let mut flat_idx = 0usize;

        for (gi, group) in self.groups.iter().enumerate() {
            if gi > 0 {
                children.push(divider());
            }
            for item in &group.items {
                let is_selected = flat_idx == self.selected_index.get();
                children.push(self.render_item(item, is_selected));
                flat_idx += 1;
            }
        }

        div()
            .id("context-menu")
            .track_focus(&self.focus)
            .focusable()
            .on_key_down(handle_key)
            .min_w(px(180.0))
            .bg(color::current().gray.s[2])
            .border_1()
            .border_color(color::current().gray.s[4])
            .rounded(radius::R4)
            .overflow_hidden()
            .py(space::S4)
            .children(children)
            .into_any_element()
    }
}
