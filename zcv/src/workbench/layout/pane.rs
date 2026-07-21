//! Pane —— 单个编辑区 Pane 的 Entity。
//!
//! 持有自己的 FocusHandle、tabs、激活状态。
//! 渲染标签栏和编辑器内容，处理键盘事件。

use gpui::{Context, FocusHandle, Render, Window, actions, div, prelude::*};
use zcv_engine::{ByteOffset, TextRange};

use crate::editor::{ViewRegistry, view::View};
use crate::shared::glyph::Glyph;
use crate::shared::icon::SvgIcon;
use crate::theme::{color, radius, space, typography};

use super::types::{PaneId, TabItem, ViewId};

actions!(pane, [CloseTab, NextTab, PrevTab]);

// ═══ 1. Struct + constructor ═══════════════════════════════════════

/// 单个编辑区 Pane。
pub(crate) struct Pane {
    pub focus: FocusHandle,
    pub id: PaneId,
    pub tabs: Vec<TabItem>,
    pub active: Option<ViewId>,
}

impl Pane {
    pub fn new(id: PaneId, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            id,
            tabs: Vec::new(),
            active: None,
        }
    }

    /// 添加一个 tab，若已存在则激活。
    pub fn add_tab(&mut self, view_id: ViewId, title: &str) {
        if let Some(_tab) = self.tabs.iter_mut().find(|t| t.view_id == view_id) {
            self.active = Some(view_id);
            return;
        }
        self.tabs.push(TabItem::new(view_id, title));
        self.active = Some(view_id);
    }

    /// 激活指定 tab。
    pub fn activate_tab(&mut self, view_id: ViewId) {
        if self.tabs.iter().any(|t| t.view_id == view_id) {
            self.active = Some(view_id);
        }
    }

    /// 切换到下一个 tab。
    pub fn next_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let pos = self
            .active
            .and_then(|id| self.tabs.iter().position(|t| t.view_id == id));
        let next = match pos {
            Some(i) if i + 1 < self.tabs.len() => i + 1,
            Some(_) => 0,
            None => 0,
        };
        self.active = Some(self.tabs[next].view_id);
    }

    /// 切换到上一个 tab。
    pub fn prev_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let pos = self
            .active
            .and_then(|id| self.tabs.iter().position(|t| t.view_id == id));
        let prev = match pos {
            Some(0) => self.tabs.len() - 1,
            Some(i) => i - 1,
            None => 0,
        };
        self.active = Some(self.tabs[prev].view_id);
    }

    /// 关闭指定 tab，自动切换到下一个。
    pub fn close_tab(&mut self, view_id: ViewId) {
        if let Some(pos) = self.tabs.iter().position(|t| t.view_id == view_id) {
            self.tabs.remove(pos);
            if self.active == Some(view_id) {
                self.active = self.tabs.last().map(|t| t.view_id);
            }
        }
    }
}

// ═══ 2. Action handler ═════════════════════════════════════════════

impl Pane {
    fn handle_next_tab(&mut self, _: &NextTab, window: &mut Window, _cx: &mut Context<Self>) {
        self.next_tab();
        window.refresh();
    }

    fn handle_prev_tab(&mut self, _: &PrevTab, window: &mut Window, _cx: &mut Context<Self>) {
        self.prev_tab();
        window.refresh();
    }
}

// ═══ 3. Render ═════════════════════════════════════════════════════

impl Render for Pane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let views = cx.global::<ViewRegistry>();
        let active_view = self.active;
        let pane_entity = cx.entity();

        div()
            .track_focus(&self.focus)
            .key_context("Pane")
            .tab_index(0)
            .flex_1()
            .flex()
            .flex_col()
            .overflow_hidden()
            .size_full()
            .bg(color::current().gray.s[1])
            .on_action(cx.listener(Self::handle_next_tab))
            .on_action(cx.listener(Self::handle_prev_tab))
            .child(render_tab_bar(&self.tabs, active_view, pane_entity))
            .child(render_content(active_view, views))
    }
}

// ═══ 4. 私有渲染辅助函数 ═══════════════════════════════════════

// ── Tab Bar ──────────────────────────────────────────────────────────

/// 标签栏：一组标签的容器。
fn render_tab_bar(
    tabs: &[TabItem],
    active_view: Option<ViewId>,
    pane_entity: gpui::Entity<Pane>,
) -> gpui::Div {
    if tabs.is_empty() {
        return div().flex_shrink_0();
    }

    div()
        .flex()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(space::S2)
        .bg(color::current().gray.s[2])
        .border_b_1()
        .border_color(color::current().gray.s[4])
        .children(
            tabs.iter()
                .map(|tab| render_tab(tab, Some(tab.view_id) == active_view, &pane_entity)),
        )
}

/// 单个标签：文件图标 + 文件名 + 关闭按钮。
fn render_tab(tab: &TabItem, is_active: bool, pane_entity: &gpui::Entity<Pane>) -> gpui::Div {
    let view_id = tab.view_id;
    let activate_entity = pane_entity.clone();
    let close_entity = pane_entity.clone();

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(space::S4)
        .p(space::S4)
        .rounded(radius::R2)
        .cursor_pointer()
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
            activate_entity.update(cx, |pane, _| pane.activate_tab(view_id));
            window.refresh();
            cx.stop_propagation();
        })
        .text_color(if is_active {
            color::current().gray.s[8]
        } else {
            color::current().gray.s[6]
        })
        .bg(if is_active {
            color::current().gray.s[1]
        } else {
            gpui::rgba(0)
        })
        .child(file_icon())
        .child(tab.title.clone())
        .child(close_glyph(&close_entity, view_id))
}

/// 文件类型图标。
fn file_icon() -> impl gpui::IntoElement {
    SvgIcon::new("icons/files/file.svg")
}

/// 标签关闭按钮（叉 glyph）。
fn close_glyph(pane_entity: &gpui::Entity<Pane>, view_id: ViewId) -> impl gpui::IntoElement {
    let entity = pane_entity.clone();
    Glyph::icon(
        ("tab-close", view_id.0),
        "icons/actions/close.svg",
        "关闭标签",
    )
    .action(CloseTab)
    .on_click(move |window: &mut gpui::Window, cx: &mut gpui::App| {
        entity.update(cx, |pane, _| pane.close_tab(view_id));
        window.refresh();
    })
}

// ── Editor Content ────────────────────────────────────────────────────

/// 渲染 Pane 内容区（编辑器内容或占位文字）。
fn render_content(active_view: Option<ViewId>, views: &ViewRegistry) -> impl gpui::IntoElement {
    let Some(view_id) = active_view else {
        return div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(color::current().gray.s[5])
            .child("无打开文件")
            .into_any_element();
    };
    let Some(view) = views.get(view_id) else {
        return div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(color::current().gray.s[5])
            .child("视图已关闭")
            .into_any_element();
    };

    render_editor_content(view).into_any_element()
}

/// 渲染编辑器内容（多行文本）。
fn render_editor_content(view: &View) -> impl gpui::IntoElement {
    let text = {
        let buf = view.buffer.borrow();
        let len = buf.len_bytes();
        if len > ByteOffset::ZERO {
            let range = TextRange::new(ByteOffset::ZERO, len);
            match range {
                Ok(r) => buf
                    .snapshot()
                    .slice_text(r)
                    .ok()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
                Err(_) => String::new(),
            }
        } else {
            String::new()
        }
    };

    let lines: Vec<&str> = text.split('\n').collect();
    if lines.is_empty() {
        return div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(color::current().gray.s[5])
            .child("空文件")
            .into_any_element();
    }

    let scroll = view.scroll_line.get() as usize;
    let scroll_idx = scroll.min(lines.len() - 1);
    const MAX_VISIBLE: usize = 200;
    let end = (scroll_idx + MAX_VISIBLE).min(lines.len());
    let view_id = view.id;

    div()
        .flex_1()
        .flex()
        .flex_col()
        .overflow_hidden()
        .font(typography::editor_font())
        .text_size(typography::editor())
        .line_height(typography::editor_line())
        .on_key_down(move |event, window, cx| {
            handle_editor_scroll(&event.keystroke, view_id, window, cx);
        })
        .children(lines[scroll_idx..end].iter().map(|line: &&str| {
            div()
                .h(typography::editor_line())
                .px(space::S4)
                .child(line.to_string())
        }))
        .into_any_element()
}

/// 处理编辑器键盘滚动。
fn handle_editor_scroll(
    keystroke: &gpui::Keystroke,
    view_id: ViewId,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let delta = match keystroke.key.as_str() {
        "up" => -1i32,
        "down" => 1,
        "pageup" => -20,
        "pagedown" => 20,
        _ => return,
    };

    cx.update_global::<ViewRegistry, _>(|reg, _| {
        if let Some(view) = reg.get_mut(view_id) {
            let current = view.scroll_line.get() as i32;
            let new = (current + delta).max(0) as u32;
            view.scroll_line.set(new);
        }
    });

    window.refresh();
}
