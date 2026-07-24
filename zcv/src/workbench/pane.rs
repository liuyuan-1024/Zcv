//! Pane —— 单个编辑区 Pane 的 Entity。
//!
//! 持有自己的 FocusHandle、tabs、激活状态。
//! 渲染标签栏和编辑器内容，处理键盘事件。

use std::collections::HashMap;

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Render, Window, actions, div, prelude::*,
};

use super::pane_group::{PaneId, TabItem, ViewId};
use crate::editor::editor::Editor;
use crate::editor::registry::ViewRegistry;
use crate::theme::{color, radius, space};
use crate::ui::glyph::Glyph;
use crate::ui::icon::SvgIcon;

actions!(pane, [CloseTab, NextTab, PrevTab]);

const TAB_HOVER_GROUP: &str = "pane.tab";

// ═══ 1. Struct + constructor ═══════════════════════════════════════

/// 单个编辑区 Pane。
pub(crate) struct Pane {
    pub focus: FocusHandle,
    pub id: PaneId,
    pub tabs: Vec<TabItem>,
    pub active: Option<ViewId>,
    editors: HashMap<ViewId, Entity<Editor>>,
}

impl Pane {
    pub fn new(id: PaneId, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            id,
            tabs: Vec::new(),
            active: None,
            editors: HashMap::new(),
        }
    }

    /// 添加一个 tab，若已存在则激活。
    pub fn add_tab(
        &mut self,
        view_id: ViewId,
        title: &str,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Editor>> {
        if !self.editors.contains_key(&view_id) {
            let buffer = cx
                .global::<ViewRegistry>()
                .get(view_id)
                .map(|view| view.buffer.clone())?;
            let editor = cx.new(|cx| Editor::for_buffer(buffer, cx));
            cx.observe(&editor, |_, _, cx| cx.notify()).detach();
            self.editors.insert(view_id, editor);
        }
        if let Some(_tab) = self.tabs.iter_mut().find(|t| t.view_id == view_id) {
            self.active = Some(view_id);
            return self.editors.get(&view_id).cloned();
        }
        self.tabs.push(TabItem::new(view_id, title));
        self.active = Some(view_id);
        self.editors.get(&view_id).cloned()
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
            self.editors.remove(&view_id);
            if self.active == Some(view_id) {
                self.active = self.tabs.last().map(|t| t.view_id);
            }
        }
    }

    pub(crate) fn active_editor(&self) -> Option<Entity<Editor>> {
        self.active
            .and_then(|view_id| self.editors.get(&view_id).cloned())
    }

    fn focus_active_editor(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.active_editor() {
            window.focus(&editor.read(cx).focus_handle());
        }
    }
}

// ═══ 2. Action handler ═════════════════════════════════════════════

impl Pane {
    fn handle_next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.next_tab();
        self.focus_active_editor(window, cx);
        window.refresh();
    }

    fn handle_prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.prev_tab();
        self.focus_active_editor(window, cx);
        window.refresh();
    }
}

// ═══ 3. Render ═════════════════════════════════════════════════════

impl Render for Pane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active_view = self.active;
        let active_editor = self.active_editor();
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
            .child(render_tab_bar(
                &self.tabs,
                &self.editors,
                active_view,
                pane_entity,
                cx,
            ))
            .child(render_content(active_view, active_editor))
    }
}

// ═══ 4. 私有渲染辅助函数 ═══════════════════════════════════════

// ── Tab Bar ──────────────────────────────────────────────────────────

/// 标签栏：一组标签的容器。
fn render_tab_bar(
    tabs: &[TabItem],
    editors: &HashMap<ViewId, Entity<Editor>>,
    active_view: Option<ViewId>,
    pane_entity: gpui::Entity<Pane>,
    cx: &App,
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
        .children(tabs.iter().map(|tab| {
            let is_dirty = editors
                .get(&tab.view_id)
                .is_some_and(|editor| editor.read(cx).is_dirty(cx));
            render_tab(
                tab,
                Some(tab.view_id) == active_view,
                is_dirty,
                &pane_entity,
                cx,
            )
        }))
}

/// 单个标签：文件图标 + 文件名 + 关闭按钮。
fn render_tab(
    tab: &TabItem,
    is_active: bool,
    is_dirty: bool,
    pane_entity: &gpui::Entity<Pane>,
    cx: &App,
) -> gpui::Div {
    let view_id = tab.view_id;
    let activate_entity = pane_entity.clone();
    let close_entity = pane_entity.clone();

    div()
        .group(TAB_HOVER_GROUP)
        .flex()
        .flex_row()
        .items_center()
        .gap(space::S4)
        .p(space::S4)
        .rounded(radius::R2)
        .cursor_pointer()
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
            let editor = activate_entity.update(cx, |pane, _| {
                pane.activate_tab(view_id);
                pane.active_editor()
            });
            if let Some(editor) = editor {
                window.focus(&editor.read(cx).focus_handle());
            }
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
        .child(tab_end_glyph(&close_entity, view_id, is_dirty, cx))
}

/// 文件类型图标。
fn file_icon() -> impl gpui::IntoElement {
    SvgIcon::new("icons/files/file.svg")
}

/// 标签关闭按钮（叉 glyph）。
fn close_glyph(
    pane_entity: &gpui::Entity<Pane>,
    view_id: ViewId,
    cx: &App,
) -> impl gpui::IntoElement {
    let entity = pane_entity.clone();
    Glyph::icon(("tab-close", view_id.0), "icons/actions/close.svg")
        .label("关闭标签")
        .shortcut(&CloseTab, cx)
        .on_click(move |window: &mut gpui::Window, cx: &mut gpui::App| {
            let editor = entity.update(cx, |pane, _| {
                pane.close_tab(view_id);
                pane.active_editor()
            });
            if let Some(editor) = editor {
                window.focus(&editor.read(cx).focus_handle());
            }
            window.refresh();
        })
}

/// 标签尾部状态槽：未保存时默认显示圆点，悬停标签后切换为关闭按钮。
fn tab_end_glyph(
    pane_entity: &gpui::Entity<Pane>,
    view_id: ViewId,
    is_dirty: bool,
    cx: &App,
) -> AnyElement {
    if !is_dirty {
        return close_glyph(pane_entity, view_id, cx).into_any_element();
    }

    let slot_size = crate::theme::typography::ui();
    div()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .size(slot_size)
        .child(
            div()
                .group_hover(TAB_HOVER_GROUP, |style| style.opacity(0.0))
                .child(
                    Glyph::icon(("tab-dirty", view_id.0), "icons/actions/circle.svg")
                        .color(color::highlight()),
                ),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .opacity(0.0)
                .group_hover(TAB_HOVER_GROUP, |style| style.opacity(1.0))
                .child(close_glyph(pane_entity, view_id, cx)),
        )
        .into_any_element()
}

// ── Editor Content ────────────────────────────────────────────────────

/// 渲染 Pane 内容区（编辑器内容或占位文字）。
fn render_content(
    active_view: Option<ViewId>,
    active_editor: Option<Entity<Editor>>,
) -> impl gpui::IntoElement {
    if active_view.is_none() {
        return div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(color::current().gray.s[5])
            .child("无打开文件")
            .into_any_element();
    }
    let Some(editor) = active_editor else {
        return div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(color::current().gray.s[5])
            .child("视图已关闭")
            .into_any_element();
    };

    div()
        .flex_1()
        .flex()
        .overflow_hidden()
        .px(space::S4)
        .child(editor)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use gpui::{AppContext, TestAppContext};
    use zcv_engine::{Buffer, BufferConfig};

    use super::*;

    #[gpui::test]
    fn pane_uses_an_editor_backed_by_the_registered_buffer(cx: &mut TestAppContext) {
        let buffer = cx.new(|_| {
            Buffer::scratch("真实编辑器".to_owned(), BufferConfig::default())
                .expect("测试 Buffer 应能创建")
        });
        let view_id = cx.update({
            let buffer = buffer.clone();
            move |cx| {
                let mut registry = ViewRegistry::new();
                let view_id = registry.register(PathBuf::from("demo.txt"), buffer);
                cx.set_global(registry);
                view_id
            }
        });
        let pane = cx.new(|cx| Pane::new(PaneId(1), cx));

        let editor = cx
            .update_entity(&pane, |pane, cx| pane.add_tab(view_id, "demo.txt", cx))
            .expect("已注册的 View 应创建 Editor");
        cx.read_entity(&editor, |editor, cx| assert!(!editor.is_dirty(cx)));
        cx.update_entity(&editor, |editor, cx| editor.set_text("阶段七", cx));
        cx.read_entity(&editor, |editor, cx| assert!(editor.is_dirty(cx)));

        cx.read_entity(&buffer, |buffer, _| {
            assert_eq!(
                buffer
                    .slice_byte_range(zcv_engine::ByteOffset::ZERO, buffer.len_bytes())
                    .expect("完整 Buffer 应可读取")
                    .as_str(),
                "阶段七"
            );
        });
        cx.read_entity(&pane, |pane, _| {
            assert_eq!(pane.active, Some(view_id));
            assert_eq!(pane.tabs.len(), 1);
            assert!(pane.active_editor().is_some());
        });
    }
}
