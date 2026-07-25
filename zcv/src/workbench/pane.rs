//! Pane —— 单个编辑区 Pane 的 Entity。
//!
//! 持有自己的 FocusHandle、tabs、激活状态。
//! 渲染标签栏和编辑器内容，处理键盘事件。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Render, Window, actions, div, prelude::*,
};
use zcv_engine::Buffer;

use super::pane_group::{PaneId, ViewId};
use crate::editor::editor::Editor;
use crate::theme::{color, radius, space};
use crate::ui::glyph::Glyph;
use crate::ui::icon::SvgIcon;

actions!(pane, [CloseTab, NextTab, PrevTab]);

const TAB_HOVER_GROUP: &str = "pane.tab";
static NEXT_VIEW_ID: AtomicU64 = AtomicU64::new(1);

// ═══ 1. Struct + constructor ═══════════════════════════════════════

/// Pane 直接拥有的标签页；文件身份和 Editor 生命周期不再经由全局 Registry 中转。
#[derive(Clone)]
pub(crate) struct TabItem {
    pub view_id: ViewId,
    pub title: String,
    pub path: PathBuf,
    pub editor: Entity<Editor>,
}

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

    /// 打开文件；当前 Pane 已有同一路径时只激活已有 Editor。
    pub fn open_file(
        &mut self,
        path: PathBuf,
        title: impl Into<String>,
        buffer: Entity<Buffer>,
        cx: &mut Context<Self>,
    ) -> Entity<Editor> {
        if let Some(tab) = self.tabs.iter().find(|tab| tab.path == path) {
            self.active = Some(tab.view_id);
            cx.notify();
            return tab.editor.clone();
        }

        let view_id = ViewId(NEXT_VIEW_ID.fetch_add(1, Ordering::Relaxed));
        let path_clone = path.clone();
        let editor = cx.new(|cx| Editor::for_buffer(buffer, cx));
        editor.update(cx, |editor, _| editor.set_file_path(path_clone));
        cx.observe(&editor, |_, _, cx| cx.notify()).detach();
        self.tabs.push(TabItem {
            view_id,
            title: title.into(),
            path,
            editor: editor.clone(),
        });
        self.active = Some(view_id);
        cx.notify();
        editor
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

    pub(crate) fn active_editor(&self) -> Option<Entity<Editor>> {
        self.active_tab().map(|tab| tab.editor.clone())
    }

    pub(crate) fn active_file(&self) -> Option<(Entity<Editor>, PathBuf)> {
        self.active_tab()
            .map(|tab| (tab.editor.clone(), tab.path.clone()))
    }

    fn active_tab(&self) -> Option<&TabItem> {
        let view_id = self.active?;
        self.tabs.iter().find(|tab| tab.view_id == view_id)
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
        cx.notify();
        window.refresh();
    }

    fn handle_prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.prev_tab();
        self.focus_active_editor(window, cx);
        cx.notify();
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
            .child(render_tab_bar(&self.tabs, active_view, pane_entity, cx))
            .child(render_content(active_view, active_editor))
    }
}

// ═══ 4. 私有渲染辅助函数 ═══════════════════════════════════════

// ── Tab Bar ──────────────────────────────────────────────────────────

/// 标签栏：一组标签的容器。
fn render_tab_bar(
    tabs: &[TabItem],
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
        .gap(space::S6)
        .bg(color::current().gray.s[2])
        .border_b_1()
        .border_color(color::current().gray.s[4])
        .children(tabs.iter().map(|tab| {
            let is_dirty = tab.editor.read(cx).is_dirty(cx);
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
        .gap(space::S6)
        .p(space::S6)
        .rounded(radius::R2)
        .cursor_pointer()
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
            let editor = activate_entity.update(cx, |pane, cx| {
                pane.activate_tab(view_id);
                cx.notify();
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
        .label("关闭")
        .shortcut(&CloseTab, cx)
        .on_click(move |window: &mut gpui::Window, cx: &mut gpui::App| {
            let editor = entity.update(cx, |pane, cx| {
                pane.close_tab(view_id);
                cx.notify();
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
        .child(editor)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext, TestAppContext};
    use zcv_engine::{Buffer, BufferConfig};

    use super::*;

    #[gpui::test]
    fn pane_owns_file_path_and_editor_backed_by_the_given_buffer(cx: &mut TestAppContext) {
        let buffer = cx.new(|_| {
            Buffer::scratch("真实编辑器".to_owned(), BufferConfig::default())
                .expect("测试 Buffer 应能创建")
        });
        let pane = cx.new(|cx| Pane::new(PaneId(1), cx));

        let editor = cx.update_entity(&pane, |pane, cx| {
            pane.open_file(PathBuf::from("demo.txt"), "demo.txt", buffer.clone(), cx)
        });
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
            assert_eq!(pane.tabs.len(), 1);
            assert_eq!(pane.tabs[0].path, PathBuf::from("demo.txt"));
            assert_eq!(pane.active, Some(pane.tabs[0].view_id));
            assert!(pane.active_editor().is_some());
        });
    }

    #[gpui::test]
    fn opening_the_same_path_reuses_the_pane_editor(cx: &mut TestAppContext) {
        let first_buffer = cx.new(|_| {
            Buffer::scratch("首次".to_owned(), BufferConfig::default()).expect("应创建 Buffer")
        });
        let second_buffer = cx.new(|_| {
            Buffer::scratch("重复".to_owned(), BufferConfig::default()).expect("应创建 Buffer")
        });
        let pane = cx.new(|cx| Pane::new(PaneId(1), cx));

        let first = cx.update_entity(&pane, |pane, cx| {
            pane.open_file(PathBuf::from("demo.txt"), "demo.txt", first_buffer, cx)
        });
        let second = cx.update_entity(&pane, |pane, cx| {
            pane.open_file(PathBuf::from("demo.txt"), "demo.txt", second_buffer, cx)
        });

        assert_eq!(first, second);
        cx.read_entity(&pane, |pane, _| assert_eq!(pane.tabs.len(), 1));
    }
}
