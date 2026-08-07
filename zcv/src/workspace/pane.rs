//! Pane —— 单个编辑区 Pane 的 Entity。
//!
//! 持有自己的 FocusHandle、tabs、激活状态。
//! 渲染标签栏和编辑器内容，处理键盘事件。
//! Pane 通过 [`ItemHandle`] trait 统操作标签页，不依赖具体视图类型。

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, App, Context, Entity, EntityId, EventEmitter, FocusHandle, Render, ScrollHandle,
    Window, actions, div, prelude::*, px,
};
use zcv_language::LanguageBuffer;

use super::item::{Item, ItemHandle};
use super::tab_bar::TabBar;
use super::toolbar::Toolbar;
use zcv_editor::{Editor, SoftWrap};
use zcv_theme::{color, typography};
use zcv_ui::Glyph;
use zcv_ui::SvgIcon;
use zcv_ui::Tab;

actions!(pane, [CloseTab, NextTab, PrevTab]);

// ═══ Pane 事件 ════════════════════════════════════════════════════════

/// Pane 对外发出的标签页事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PaneEvent {
    /// 新标签页添加。
    Add { item_id: EntityId },
    /// 活动标签页切换。
    Activate { item_id: EntityId },
    /// 标签页被关闭。
    Removed { item_id: EntityId },
}

impl EventEmitter<PaneEvent> for Pane {}

const TAB_HOVER_GROUP: &str = "pane.tab";

// ═══ DraggedTab —— 拖拽载荷 + 幽灵视图 ═════════════════════════════

/// 拖拽过程中传递的数据，同时也是拖拽时跟随鼠标的幽灵视图。
///
/// 仅支持同 Pane 内拖拽（drop 目标绑定在当前 Pane 的标签容器上）。
/// `pane` 引用只用于幽灵视图读取标签数据，不参与 drop 的跨 Pane 判断（跨 Pane 拖拽在 v1 不支持）。
#[derive(Clone)]
pub(crate) struct DraggedTab {
    pub pane: Entity<Pane>,
    pub item_id: EntityId,
    pub ix: usize,
    pub is_active: bool,
}

impl Render for DraggedTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, is_dirty, icon_path) = self
            .pane
            .read(cx)
            .tabs
            .get(self.ix)
            .map(|item| {
                (
                    item.tab_content_text(cx),
                    item.is_dirty(cx),
                    item.file_path(cx),
                )
            })
            .unwrap_or_default();

        Tab::new("")
            .selected(self.is_active)
            .start_slot(file_icon(icon_path.as_deref()))
            .end_slot(tab_end_glyph(&self.pane, self.item_id, is_dirty, cx))
            .child(title)
    }
}

// ═══ 1. Struct + constructor ═══════════════════════════════════════

/// 单个编辑区 Pane。
pub(crate) struct Pane {
    pub focus: FocusHandle,
    pub tabs: Vec<Box<dyn ItemHandle>>,
    pub active: Option<EntityId>,
    toolbar: Entity<Toolbar>,
    scroll_handle: ScrollHandle,
}

impl Pane {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            tabs: Vec::new(),
            active: None,
            toolbar: cx.new(|_| Toolbar::new()),
            scroll_handle: ScrollHandle::new(),
        }
    }

    /// 滚动到指定索引的标签到可视区域。
    fn scroll_to_tab(&self, ix: usize) {
        self.scroll_handle.scroll_to_item(ix);
    }

    /// 把任意 Item Entity 加入 Pane 并激活。
    pub(crate) fn add_item<T: Item>(
        &mut self,
        item: Entity<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        cx.observe(&item, |_, _, cx| cx.notify()).detach();

        let item: Box<dyn ItemHandle> = Box::new(item);
        let item_id = item.item_id();
        let focus = item.item_focus_handle(cx);
        self.tabs.push(item);
        self.active = Some(item_id);
        self.scroll_to_tab(self.tabs.len() - 1);
        self.update_toolbar(window, cx);
        cx.emit(PaneEvent::Add { item_id });
        cx.emit(PaneEvent::Activate { item_id });
        cx.notify();
        focus
    }

    /// 打开文件；当前 Pane 已有同一路径时只激活已有 Editor。
    /// 返回新标签的焦点句柄，供调用方聚焦。
    pub fn open_file(
        &mut self,
        path: PathBuf,
        project_root: PathBuf,
        buffer: Entity<LanguageBuffer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        // 已有此文件时只激活
        if let Some(item) = self
            .tabs
            .iter()
            .find(|item| item.file_path(cx).as_deref() == Some(path.as_path()))
        {
            let item_id = item.item_id();
            self.active = Some(item_id);
            cx.emit(PaneEvent::Activate { item_id });
            cx.notify();
            return item.item_focus_handle(cx);
        }

        let editor = cx.new(|cx| Editor::for_buffer(buffer, cx));
        editor.update(cx, |editor, cx| {
            editor.set_file_path(path, project_root, cx);
        });
        self.add_item(editor, window, cx)
    }

    pub(crate) fn set_soft_wrap(
        &mut self,
        soft_wrap: SoftWrap,
        preferred_line_length: usize,
        cx: &mut Context<Self>,
    ) {
        // 软换行是 Editor 能力，经 ItemHandle 接口分发，Pane 不感知具体 item 类型。
        for item in &self.tabs {
            item.set_soft_wrap(soft_wrap, preferred_line_length, cx);
        }
    }

    /// 将已打开编辑器的文件路径随文件或目录重命名一起迁移。
    pub(crate) fn rename_path(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        for item in &self.tabs {
            item.rename_path(from, to, cx);
        }
        cx.notify();
    }

    /// 关闭已删除条目对应的标签页；目录删除时连同其中打开的文件一起关闭。
    pub(crate) fn remove_path(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let closed: Vec<EntityId> = self
            .tabs
            .iter()
            .filter_map(|item| {
                let open_path = item.file_path(cx)?;
                open_path.strip_prefix(path).is_ok().then(|| item.item_id())
            })
            .collect();
        // close_tab 逐个发射 Removed，订阅方（项目树高亮）自动刷新。
        for item_id in closed {
            self.close_tab(item_id, window, cx);
        }
    }

    /// 激活指定 tab，并滚入视图。
    pub fn activate_tab(&mut self, item_id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pos) = self.tabs.iter().position(|item| item.item_id() == item_id) {
            self.active = Some(item_id);
            self.scroll_to_tab(pos);
            self.update_toolbar(window, cx);
        }
    }

    /// 切换到下一个 tab，并滚入视图。
    pub fn next_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let pos = self
            .active
            .and_then(|id| self.tabs.iter().position(|item| item.item_id() == id));
        let next = match pos {
            Some(i) if i + 1 < self.tabs.len() => i + 1,
            Some(_) => 0,
            None => 0,
        };
        self.active = Some(self.tabs[next].item_id());
        self.scroll_to_tab(next);
    }

    /// 切换到上一个 tab，并滚入视图。
    pub fn prev_tab(&mut self) {
        if self.tabs.is_empty() {
            return;
        }
        let pos = self
            .active
            .and_then(|id| self.tabs.iter().position(|item| item.item_id() == id));
        let prev = match pos {
            Some(0) => self.tabs.len() - 1,
            Some(i) => i - 1,
            None => 0,
        };
        self.active = Some(self.tabs[prev].item_id());
        self.scroll_to_tab(prev);
    }

    /// 关闭指定 tab，激活原位置的下一个；统一在此发射 `Removed` 事件。
    ///
    /// 关闭的是最后一项时激活新的最后一项；
    /// 所有关闭路径（快捷键、关闭按钮、删除文件）都收敛到本方法，订阅方只需监听 Pane 事件。
    pub fn close_tab(&mut self, item_id: EntityId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pos) = self.tabs.iter().position(|item| item.item_id() == item_id) {
            self.tabs.remove(pos);
            if self.active == Some(item_id) {
                self.active = self
                    .tabs
                    .get(pos)
                    .map(|item| item.item_id())
                    .or_else(|| self.tabs.last().map(|item| item.item_id()));
                self.update_toolbar(window, cx);
            }
            cx.emit(PaneEvent::Removed { item_id });
            cx.notify();
        }
    }

    /// 当前活动标签的 ItemHandle。
    pub(crate) fn active_item(&self) -> Option<&dyn ItemHandle> {
        let item_id = self.active?;
        self.tabs
            .iter()
            .find(|item| item.item_id() == item_id)
            .map(|item| item.as_ref())
    }

    /// 按具体 Item 类型获取活动标签。
    pub(crate) fn active_item_as<T: Render + 'static>(&self) -> Option<Entity<T>> {
        self.active_item()?.downcast()
    }

    /// 向下转型获取活动编辑器（仅供需要 Editor 的场合使用）。
    pub(crate) fn active_editor(&self) -> Option<Entity<Editor>> {
        self.active_item_as()
    }

    /// 活动编辑器的路径（如果有）。
    pub(crate) fn active_path(&self, cx: &App) -> Option<PathBuf> {
        self.active_item()?.file_path(cx)
    }

    fn focus_active_item(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.active_item() {
            window.focus(&item.item_focus_handle(cx));
        }
    }

    /// 返回 Toolbar Entity 的引用，供 Workspace 注册子项。
    pub(crate) fn toolbar(&self) -> &Entity<Toolbar> {
        &self.toolbar
    }

    /// 根据当前激活的 item 更新 Toolbar 内容。
    fn update_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_item = self.active_item();
        self.toolbar.update(cx, |toolbar, cx| {
            toolbar.set_active_item(active_item, window, cx);
        });
    }
}

// ═══ 拖拽重排序 ═══════════════════════════════════════════════════

impl Pane {
    /// 在同 Pane 内移动标签页从 `from_ix` 到 `to_ix`（`to_ix` 是最终数组位置）。
    fn move_tab(&mut self, from_ix: usize, to_ix: usize) {
        if from_ix == to_ix {
            return;
        }
        let tab = self.tabs.remove(from_ix);
        self.tabs.insert(to_ix.min(self.tabs.len()), tab);
    }

    /// 处理标签拖拽放置（drop 目标在本 Pane 内，天然同 Pane）。
    pub(crate) fn handle_tab_drop(
        &mut self,
        dragged: &DraggedTab,
        target_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_tab(dragged.ix, target_ix);
        self.update_toolbar(window, cx);
        // 保持或恢复激活状态
        if let Some(item) = self
            .tabs
            .iter()
            .find(|item| item.item_id() == dragged.item_id)
        {
            self.active = Some(item.item_id());
        }
        cx.emit(PaneEvent::Activate {
            item_id: self.active.unwrap_or(dragged.item_id),
        });
        cx.notify();
    }
}

// ═══ 2. Action handler ═════════════════════════════════════════════

impl Pane {
    fn handle_next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.next_tab();
        // 关闭最后一个 tab 后按快捷键会走到这里：next_tab 对空 tabs 早退，active 可能为 None。
        let Some(item_id) = self.active else {
            return;
        };
        self.update_toolbar(window, cx);
        self.focus_active_item(window, cx);
        cx.emit(PaneEvent::Activate { item_id });
        cx.notify();
        window.refresh();
    }

    fn handle_prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.prev_tab();
        let Some(item_id) = self.active else {
            return;
        };
        self.update_toolbar(window, cx);
        self.focus_active_item(window, cx);
        cx.emit(PaneEvent::Activate { item_id });
        cx.notify();
        window.refresh();
    }
}

// ═══ 3. Render ═════════════════════════════════════════════════════

impl Render for Pane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active_item_id = self.active;
        let active_item = self.active_item();
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
            .bg(color::current(cx).editor_background)
            .on_action(cx.listener(Self::handle_next_tab))
            .on_action(cx.listener(Self::handle_prev_tab))
            .child(render_tab_bar(
                &self.tabs,
                active_item_id,
                pane_entity,
                &self.scroll_handle,
                cx,
            ))
            .child(self.toolbar.clone())
            .child(render_content(active_item_id, active_item, cx))
    }
}

// ═══ 4. 私有渲染辅助函数 ═══════════════════════════════════════

// ── Tab Bar ──────────────────────────────────────────────────────────

/// 标签栏：一组标签的容器 + 末尾放置目标。
fn render_tab_bar(
    tabs: &[Box<dyn ItemHandle>],
    active_item_id: Option<EntityId>,
    pane_entity: gpui::Entity<Pane>,
    scroll_handle: &ScrollHandle,
    cx: &App,
) -> impl gpui::IntoElement {
    let children: Vec<AnyElement> = tabs
        .iter()
        .enumerate()
        .map(|(ix, item)| {
            let is_dirty = item.is_dirty(cx);
            render_tab(
                item.as_ref(),
                ix,
                Some(item.item_id()) == active_item_id,
                is_dirty,
                &pane_entity,
                cx,
            )
            .into_any_element()
        })
        .chain(std::iter::once(
            render_tab_bar_drop_target(&pane_entity, tabs.len(), cx).into_any_element(),
        ))
        .collect();

    let handle = scroll_handle.clone();
    let tab_bar = TabBar::new().track_scroll(scroll_handle).with_bar(
        cx,
        |bar| {
            bar.flex()
                .flex_row()
                .items_center()
                .flex_shrink_0()
                .bg(color::current(cx).tab_bar_background)
        },
        children,
    );

    // 外层包裹 on_drag_move 实现拖拽到边缘自动滚动 event.bounds 就是本 div 的边界，无需 Y 坐标判断
    div()
        .id("tab-bar-area")
        .flex_shrink_0()
        .child(tab_bar)
        .on_drag_move::<DraggedTab>(move |event, window, _cx| {
            let margin = px(30.0);
            let mouse_x = event.event.position.x;
            let left = event.bounds.left();
            let right = event.bounds.right();

            let mut offset = handle.offset();
            if mouse_x < left + margin {
                offset.x = (offset.x + px(8.0)).min(px(0.0));
                handle.set_offset(offset);
                window.refresh();
            } else if mouse_x > right - margin {
                let max_x = handle.max_offset().width;
                offset.x = (offset.x - px(8.0)).max(-max_x);
                handle.set_offset(offset);
                window.refresh();
            }
        })
}

/// 单个标签：文件图标 + 文件名 + 关闭按钮，支持拖拽重排序。
fn render_tab(
    item: &dyn ItemHandle,
    ix: usize,
    is_active: bool,
    is_dirty: bool,
    pane_entity: &gpui::Entity<Pane>,
    cx: &App,
) -> impl gpui::IntoElement {
    let item_id = item.item_id();
    let activate_entity = pane_entity.clone();
    let close_entity = pane_entity.clone();

    Tab::new(("tab", item_id))
        .selected(is_active)
        .start_slot(file_icon(item.file_path(cx).as_deref()))
        .end_slot(tab_end_glyph(&close_entity, item_id, is_dirty, cx))
        .child(item.tab_content_text(cx))
        .group(TAB_HOVER_GROUP)
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
            let focus = activate_entity.update(cx, |pane, cx| {
                pane.activate_tab(item_id, window, cx);
                cx.emit(PaneEvent::Activate { item_id });
                cx.notify();
                pane.active_item().map(|item| item.item_focus_handle(cx))
            });
            if let Some(focus) = focus {
                window.focus(&focus);
            }
            window.refresh();
            cx.stop_propagation();
        })
        .on_drag(
            DraggedTab {
                pane: pane_entity.clone(),
                item_id,
                ix,
                is_active,
            },
            |tab, _, _, cx| cx.new(|_| tab.clone()),
        )
        .drag_over::<DraggedTab>(
            move |mut tab: gpui::StyleRefinement, dragged: &DraggedTab, _, cx| {
                if ix != dragged.ix {
                    tab.background = Some(gpui::Fill::from(color::current(cx).element_hover));
                }
                tab
            },
        )
        .on_drop({
            let pane = pane_entity.clone();
            move |dragged: &DraggedTab, window, cx| {
                pane.update(cx, |this, cx| {
                    this.handle_tab_drop(dragged, ix, window, cx);
                });
            }
        })
}

/// 标签栏末尾的放置目标：将标签拖到所有标签末尾时接受放置。
fn render_tab_bar_drop_target(
    pane_entity: &gpui::Entity<Pane>,
    tab_count: usize,
    _cx: &App,
) -> impl gpui::IntoElement {
    let pane = pane_entity.clone();
    div()
        .id("tab-bar-drop-target")
        .flex_grow()
        .drag_over::<DraggedTab>(
            |mut tab: gpui::StyleRefinement, _dragged: &DraggedTab, _, cx| {
                tab.background = Some(gpui::Fill::from(color::current(cx).element_hover));
                tab
            },
        )
        .on_drop(move |dragged: &DraggedTab, window, cx| {
            pane.update(cx, |this, cx| {
                this.handle_tab_drop(dragged, tab_count, window, cx);
            });
        })
}

/// 按路径渲染文件类型图标：目录用 folder 图标，文件用通用 file 图标。
///
/// 未来扩展按扩展名区分的图标库时，在本函数内追加映射即可。
fn file_icon(path: Option<&Path>) -> impl gpui::IntoElement {
    let icon = match path {
        Some(path) if path.is_dir() => "icons/folder.svg",
        _ => "icons/file.svg",
    };
    SvgIcon::new(icon)
}

/// 标签关闭按钮（叉 glyph）。
fn close_glyph(
    pane_entity: &gpui::Entity<Pane>,
    item_id: EntityId,
    cx: &App,
) -> impl gpui::IntoElement {
    let entity = pane_entity.clone();
    Glyph::icon(("tab-close", item_id), "icons/close.svg")
        .label("关闭")
        .shortcut(&CloseTab, cx)
        .on_click(move |window: &mut gpui::Window, cx: &mut gpui::App| {
            let pane_focus = entity.read(cx).focus.clone();
            let focus = entity.update(cx, |pane, cx| {
                pane.close_tab(item_id, window, cx);
                pane.active_item().map(|item| item.item_focus_handle(cx))
            });
            window.focus(&focus.unwrap_or(pane_focus));
            window.refresh();
        })
}

/// 标签尾部状态槽：未保存时默认显示圆点，悬停标签后切换为关闭按钮。
fn tab_end_glyph(
    pane_entity: &gpui::Entity<Pane>,
    item_id: EntityId,
    is_dirty: bool,
    cx: &App,
) -> AnyElement {
    if !is_dirty {
        return close_glyph(pane_entity, item_id, cx).into_any_element();
    }

    let slot_size = typography::ui();
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
                    Glyph::icon(("tab-dirty", item_id), "icons/circle.svg")
                        .color(color::current(cx).icon_accent),
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
                .child(close_glyph(pane_entity, item_id, cx)),
        )
        .into_any_element()
}

// ── Editor Content ────────────────────────────────────────────────────

/// 渲染 Pane 内容区（编辑器内容或占位文字）。
fn render_content(
    active_item_id: Option<EntityId>,
    active_item: Option<&dyn ItemHandle>,
    cx: &App,
) -> impl gpui::IntoElement {
    if active_item_id.is_none() {
        return div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(color::current(cx).text_placeholder)
            .child("无打开文件")
            .into_any_element();
    }
    let Some(item) = active_item else {
        return div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .text_color(color::current(cx).text_placeholder)
            .child("视图已关闭")
            .into_any_element();
    };

    div()
        .flex_1()
        .flex()
        .overflow_hidden()
        .child(item.to_any_view())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{Context, Render, TestAppContext, Window, div, prelude::*};
    use zcv_engine::{Buffer, BufferConfig};

    use super::*;

    /// 辅助视图类型，仅用于测试中创建窗口。
    struct TestView;
    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    /// 辅助：在测试中用 add_window_view 提供 window 上下文打开文件。
    fn open_file_in_test(
        cx: &mut TestAppContext,
        pane: &Entity<Pane>,
        path: PathBuf,
        buffer: Entity<LanguageBuffer>,
    ) {
        cx.add_window_view(|window, cx| {
            pane.update(cx, |p, cx| {
                p.open_file(
                    path,
                    std::env::current_dir().expect("测试项目根应可读取"),
                    buffer,
                    window,
                    cx,
                );
            });
            TestView
        });
    }

    fn test_buffer(cx: &mut TestAppContext, text: impl Into<String>) -> Entity<LanguageBuffer> {
        let buffer = cx.new(|_| {
            Buffer::scratch(text.into(), BufferConfig::default()).expect("应创建测试 Buffer")
        });
        cx.new(|cx| LanguageBuffer::new(buffer, None, cx))
    }

    #[gpui::test]
    fn pane_owns_file_path_and_editor_backed_by_the_given_buffer(cx: &mut TestAppContext) {
        let raw_buffer = cx.new(|_| {
            Buffer::scratch("真实编辑器".to_owned(), BufferConfig::default())
                .expect("测试 Buffer 应能创建")
        });
        let buffer = cx.new(|cx| LanguageBuffer::new(raw_buffer.clone(), None, cx));
        let pane = cx.new(Pane::new);
        open_file_in_test(cx, &pane, PathBuf::from("demo.txt"), buffer.clone());

        let editor = cx.read_entity(&pane, |pane, _| pane.active_editor().unwrap());
        cx.read_entity(&editor, |editor, cx| assert!(!editor.is_dirty(cx)));
        cx.update_entity(&editor, |editor, cx| editor.set_text("阶段七", cx));
        cx.read_entity(&editor, |editor, cx| assert!(editor.is_dirty(cx)));

        cx.read_entity(&raw_buffer, |buffer, _| {
            assert_eq!(
                buffer
                    .slice_byte_range(zcv_engine::ByteOffset::ZERO, buffer.len_bytes())
                    .expect("完整 Buffer 应可读取")
                    .as_str(),
                "阶段七"
            );
        });
        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 1);
            assert_eq!(
                pane.tabs[0]
                    .file_path(cx)
                    .as_deref()
                    .map(|p| p.to_string_lossy().to_string()),
                Some("demo.txt".to_string())
            );
            assert_eq!(pane.active, Some(pane.tabs[0].item_id()));
            assert!(pane.active_editor().is_some());
        });
    }

    #[gpui::test]
    fn opening_the_same_path_reuses_the_pane_editor(cx: &mut TestAppContext) {
        let first_buffer = test_buffer(cx, "首次");
        let second_buffer = test_buffer(cx, "重复");

        let pane = cx.new(Pane::new);
        open_file_in_test(cx, &pane, PathBuf::from("demo.txt"), first_buffer);
        open_file_in_test(cx, &pane, PathBuf::from("demo.txt"), second_buffer);

        // 同一路径不应创建重复标签
        cx.read_entity(&pane, |pane, _| assert_eq!(pane.tabs.len(), 1));
    }

    #[gpui::test]
    fn move_tab_reorders_tabs_correctly(cx: &mut TestAppContext) {
        let pane = cx.new(Pane::new);

        // 用 scratch Buffer 模拟多个标签
        for i in 0..4 {
            let buffer = test_buffer(cx, format!("内容{i}"));
            let path = PathBuf::from(format!("file{i}.txt"));
            open_file_in_test(cx, &pane, path, buffer);
        }

        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 4);
            assert_eq!(pane.tabs[0].tab_content_text(cx).as_ref(), "file0.txt");
            assert_eq!(pane.tabs[1].tab_content_text(cx).as_ref(), "file1.txt");
            assert_eq!(pane.tabs[2].tab_content_text(cx).as_ref(), "file2.txt");
            assert_eq!(pane.tabs[3].tab_content_text(cx).as_ref(), "file3.txt");
        });

        // 移动：将索引 2 移到索引 0
        cx.update_entity(&pane, |pane, _| pane.move_tab(2, 0));
        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 4);
            assert_eq!(pane.tabs[0].tab_content_text(cx).as_ref(), "file2.txt");
            assert_eq!(pane.tabs[1].tab_content_text(cx).as_ref(), "file0.txt");
            assert_eq!(pane.tabs[2].tab_content_text(cx).as_ref(), "file1.txt");
            assert_eq!(pane.tabs[3].tab_content_text(cx).as_ref(), "file3.txt");
        });

        // 移动：将索引 0 移到索引 3（拖到末尾）
        cx.update_entity(&pane, |pane, _| pane.move_tab(0, 3));
        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 4);
            assert_eq!(pane.tabs[0].tab_content_text(cx).as_ref(), "file0.txt");
            assert_eq!(pane.tabs[1].tab_content_text(cx).as_ref(), "file1.txt");
            assert_eq!(pane.tabs[2].tab_content_text(cx).as_ref(), "file3.txt");
            assert_eq!(pane.tabs[3].tab_content_text(cx).as_ref(), "file2.txt");
        });

        // 移动：不动（自身）
        cx.update_entity(&pane, |pane, _| pane.move_tab(1, 1));
        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 4);
            assert_eq!(pane.tabs[0].tab_content_text(cx).as_ref(), "file0.txt");
        });

        // 移动：单标签拖到末尾 → 不应闪退
        let single_pane = cx.new(Pane::new);
        let buffer = test_buffer(cx, "仅一个标签");
        open_file_in_test(cx, &single_pane, PathBuf::from("solo.txt"), buffer);
        cx.read_entity(&single_pane, |pane, _cx| {
            assert_eq!(pane.tabs.len(), 1);
        });
        // 拖到自身（from == to）— 无操作
        cx.update_entity(&single_pane, |pane, _| pane.move_tab(0, 0));
        cx.read_entity(&single_pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 1);
            assert_eq!(pane.tabs[0].tab_content_text(cx).as_ref(), "solo.txt");
        });
        // 拖到末尾（to_ix 超出范围）— clamp 后不应闪退
        cx.update_entity(&single_pane, |pane, _| pane.move_tab(0, 1));
        cx.read_entity(&single_pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 1);
            assert_eq!(pane.tabs[0].tab_content_text(cx).as_ref(), "solo.txt");
        });
    }

    #[gpui::test]
    fn every_close_path_emits_removed(cx: &mut TestAppContext) {
        // 回归：三条关闭路径（close_tab 直接关闭、删除文件触发）都必须发射 Removed，
        // 订阅方（项目树高亮）才能刷新。
        let buffer = test_buffer(cx, "内容");
        let pane = cx.new(Pane::new);
        open_file_in_test(cx, &pane, PathBuf::from("a.txt"), buffer);
        let item_id = cx.read_entity(&pane, |pane, _| pane.active.unwrap());

        let removed = Rc::new(RefCell::new(Vec::new()));
        let observed = Rc::clone(&removed);
        let _subscription = cx.update(|cx| {
            cx.subscribe(&pane, move |_, event, _| {
                if matches!(event, PaneEvent::Removed { .. }) {
                    observed.borrow_mut().push(*event);
                }
            })
        });

        // 路径 1：close_tab 直接关闭 → Removed。
        cx.add_window_view(|window, cx| {
            pane.update(cx, |pane, cx| {
                pane.close_tab(item_id, window, cx);
            });
            TestView
        });
        assert_eq!(removed.borrow().len(), 1, "close_tab 应发射 Removed");

        // 路径 2：删除文件触发 remove_path 关闭 → Removed。
        let buffer = test_buffer(cx, "内容");
        open_file_in_test(cx, &pane, PathBuf::from("b.txt"), buffer);
        cx.add_window_view(|window, cx| {
            pane.update(cx, |pane, cx| {
                pane.remove_path(Path::new("b.txt"), window, cx);
            });
            TestView
        });
        assert_eq!(
            removed.borrow().len(),
            2,
            "remove_path 关闭 tab 应发射 Removed"
        );
        cx.read_entity(&pane, |pane, _| assert!(pane.tabs.is_empty()));
    }
}
