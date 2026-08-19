//! Pane —— 单个编辑区 Pane 的 Entity。
//!
//! 持有自己的 FocusHandle、tabs、激活状态。
//! 渲染标签栏和编辑器内容，处理键盘事件。
//! Pane 通过 [`ItemHandle`] trait 统操作标签页，不依赖具体视图类型。

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, App, Context, Entity, EntityId, EventEmitter, FocusHandle, Render, ScrollHandle,
    Window, div, prelude::*, px,
};
use zcv_actions::{CloseTab, DeploySearch, NextTab, PrevTab, TogglePreview};
use zcv_theme::{FileIcons, color, typography};
use zcv_ui::{Glyph, SvgIcon, Tab};

use crate::preview::{PreviewDocument, provider_for};
use crate::search_bar::SearchBar;
use crate::tab_bar::TabBar;
use crate::toolbar::Toolbar;
use crate::{ItemEvent, ItemHandle};

// ═══ Pane 事件 ════════════════════════════════════════════════════════

/// Pane 对外发出的标签页事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneEvent {
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
pub struct DraggedTab {
    pub pane: Entity<Pane>,
    pub item_id: EntityId,
    pub ix: usize,
    pub is_active: bool,
}

impl Render for DraggedTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, item) = self
            .pane
            .read(cx)
            .tabs
            .get(self.ix)
            .map(|item| (item.tab_content_text(0, cx), Some(item.boxed_clone())))
            .unwrap_or_default();
        let is_transient = self.pane.read(cx).transient_item_id == Some(self.item_id);
        Tab::new("")
            .selected(self.is_active)
            .italic(is_transient)
            .start_slot(item_icon(item.as_deref(), cx))
            .end_slot(tab_end_glyph(
                &self.pane,
                self.item_id,
                item.as_deref().is_some_and(|item| item.is_dirty(cx)),
                item.as_deref()
                    .is_some_and(|item| is_preview_item(item, cx)),
                cx,
            ))
            .child(title)
    }
}

// ═══ Pane 实体 ══════════════════════════════════════════════════

/// 单个编辑区 Pane。
pub struct Pane {
    pub focus: FocusHandle,
    pub tabs: Vec<Box<dyn ItemHandle>>,
    pub active: Option<EntityId>,
    /// 当前唯一的临时标签；固定打开或发生编辑时提升为固定标签。
    transient_item_id: Option<EntityId>,
    toolbar: Entity<Toolbar>,
    search_bar: Entity<SearchBar>,
    scroll_handle: ScrollHandle,
}

impl Pane {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let search_bar = cx.new(SearchBar::new);
        let toolbar = cx.new(|_| {
            let mut toolbar = Toolbar::new();
            toolbar.set_search_bar(search_bar.clone());
            toolbar
        });
        Self {
            focus: cx.focus_handle(),
            tabs: Vec::new(),
            active: None,
            transient_item_id: None,
            search_bar,
            toolbar,
            scroll_handle: ScrollHandle::new(),
        }
    }

    /// 滚动到指定索引的标签到可视区域。
    fn scroll_to_tab(&self, ix: usize) {
        self.scroll_handle.scroll_to_item(ix);
    }

    fn add_boxed_item_at(
        &mut self,
        item: Box<dyn ItemHandle>,
        destination_index: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        let item_id = item.item_id();
        // 订阅 Item 事件：编辑提升临时标签、标题刷新。
        // CloseItem（需 window 才能关闭 tab）暂不处理，后续对齐 Zed 的订阅签名时实现。
        let pane = cx.entity().downgrade();
        item.subscribe_to_item_events(
            cx,
            Box::new(move |event, cx| {
                let Some(pane) = pane.upgrade() else {
                    return;
                };
                pane.update(cx, |pane, cx| match event {
                    // 临时标签一旦关联文档发生编辑，就提升为固定标签。
                    ItemEvent::Edit => {
                        if pane.transient_item_id == Some(item_id) {
                            pane.transient_item_id = None;
                        }
                        // 固定标签同样需要立即重绘未保存标记。
                        cx.notify();
                    }
                    ItemEvent::UpdateTab => cx.notify(),
                    _ => {}
                });
            }),
        )
        .detach();
        let focus = item.item_focus_handle(cx);
        let index = destination_index
            .unwrap_or(self.tabs.len())
            .min(self.tabs.len());
        self.tabs.insert(index, item);
        self.active = Some(item_id);
        self.scroll_to_tab(index);
        self.update_toolbar(window, cx);
        cx.emit(PaneEvent::Add { item_id });
        cx.emit(PaneEvent::Activate { item_id });
        cx.notify();
        focus
    }

    /// 在原标签位置切换展示 Item，并保持临时/固定生命周期。
    fn replace_item_at(
        &mut self,
        index: usize,
        item: Box<dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        let old_id = self.tabs[index].item_id();
        let was_transient = self.transient_item_id == Some(old_id);
        self.close_tab(old_id, window, cx);
        let new_id = item.item_id();
        let focus = self.add_boxed_item_at(item, Some(index), window, cx);
        if was_transient {
            self.transient_item_id = Some(new_id);
            cx.notify();
        }
        focus
    }

    /// 打开一个 Item；`allow_transient` 为 true 时创建可被下一个单击替换的临时标签。
    /// 单击产生临时标签；支持预览的格式默认显示预览内容，双击文件则固定源码。
    pub fn open_item(
        &mut self,
        item: Box<dyn ItemHandle>,
        allow_transient: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        if let Some(path) = item.file_path(cx)
            && let Some(index) = self
                .tabs
                .iter()
                .position(|tab| tab.file_path(cx).as_deref() == Some(path.as_path()))
        {
            let existing = self.tabs[index].as_ref();
            let item_id = existing.item_id();
            let is_preview = is_preview_item(existing, cx);

            // 双击文件永远打开固定源码；若单击阶段是渲染内容，就在原位换成源码 Item。
            if !allow_transient && is_preview {
                let source = existing
                    .as_preview_item(cx)
                    .and_then(|preview| preview.source_item(cx))
                    .unwrap_or_else(|| item.boxed_clone());
                self.promote_transient_tab(item_id, cx);
                return self.replace_item_at(index, source, window, cx);
            }

            let focus = existing.item_focus_handle(cx);
            if !allow_transient {
                self.promote_transient_tab(item_id, cx);
            }
            self.active = Some(item_id);
            self.update_toolbar(window, cx);
            cx.emit(PaneEvent::Activate { item_id });
            cx.notify();
            return focus;
        }

        let transient_index = if allow_transient {
            self.take_replaceable_transient(window, cx)
        } else {
            None
        };

        // 单击打开支持预览的格式时，用预览视图替换源码 Item 的展示。
        if allow_transient
            && let Some(path) = item.file_path(cx)
            && let Some(provider) = provider_for(&path, cx)
        {
            let document = PreviewDocument {
                path,
                source_item: item,
            };
            let preview = provider.create(document, cx);
            let item_id = preview.item_id();
            let focus = self.add_boxed_item_at(preview, transient_index, window, cx);
            self.transient_item_id = Some(item_id);
            cx.notify();
            return focus;
        }

        let item_id = item.item_id();
        let focus = self.add_boxed_item_at(item, transient_index, window, cx);
        if allow_transient {
            self.transient_item_id = Some(item_id);
            cx.notify();
        }
        focus
    }

    /// 在源码与渲染表现之间切换；只替换展示 Item，标签生命周期保持不变。
    pub fn toggle_preview(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<FocusHandle> {
        let item_id = self.active?;
        let index = self
            .tabs
            .iter()
            .position(|item| item.item_id() == item_id)?;
        let active_item = self.tabs[index].as_ref();
        let next_item: Box<dyn ItemHandle> = if is_preview_item(active_item, cx) {
            // 预览 Item 暴露其源码 Item；无法暴露时保持当前展示。
            active_item
                .as_preview_item(cx)
                .and_then(|preview| preview.source_item(cx))
                .unwrap_or_else(|| active_item.boxed_clone())
        } else {
            let path = active_item.file_path(cx)?;
            let provider = provider_for(&path, cx)?;
            provider.create(
                PreviewDocument {
                    path,
                    source_item: active_item.boxed_clone(),
                },
                cx,
            )
        };
        let focus = self.replace_item_at(index, next_item, window, cx);
        window.focus(&focus);
        window.refresh();
        Some(focus)
    }

    /// 移除可安全替换的临时标签并返回其位置；脏标签会先提升为固定标签。
    fn take_replaceable_transient(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let item_id = self.transient_item_id.take()?;
        let index = self
            .tabs
            .iter()
            .position(|item| item.item_id() == item_id)?;
        if self.tabs[index].is_dirty(cx) {
            return None;
        }
        self.close_tab(item_id, window, cx);
        Some(index)
    }

    /// 将临时标签固定为普通标签；内容视图状态保持不变。
    fn promote_transient_tab(&mut self, item_id: EntityId, cx: &mut Context<Self>) {
        if self.transient_item_id == Some(item_id) {
            self.transient_item_id = None;
            cx.notify();
        }
    }

    /// 当前打开的所有标签（供宿主按具体 Item 类型操作）。
    pub fn tabs(&self) -> &[Box<dyn ItemHandle>] {
        &self.tabs
    }

    /// 将已打开编辑器的文件路径随文件或目录重命名一起迁移。
    pub fn rename_path(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        for item in &self.tabs {
            item.rename_path(from, to, cx);
        }
        cx.notify();
    }

    /// 关闭已删除条目对应的标签页；目录删除时连同其中打开的文件一起关闭。
    pub fn remove_path(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
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
            if self.transient_item_id == Some(item_id) {
                self.transient_item_id = None;
            }
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
    pub fn active_item(&self) -> Option<&dyn ItemHandle> {
        let item_id = self.active?;
        self.tabs
            .iter()
            .find(|item| item.item_id() == item_id)
            .map(|item| item.as_ref())
    }

    /// 活动编辑器的路径（如果有）。
    pub fn active_path(&self, cx: &App) -> Option<PathBuf> {
        self.active_item()?.file_path(cx)
    }

    fn focus_active_item(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.active_item() {
            window.focus(&item.item_focus_handle(cx));
        }
    }

    /// 返回 Toolbar Entity 的引用，供 Workspace 注册子项。
    pub fn toolbar(&self) -> &Entity<Toolbar> {
        &self.toolbar
    }

    /// 根据当前激活的 item 更新 Toolbar 内容。
    fn update_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_item = self.active_item();
        self.toolbar.update(cx, |toolbar, cx| {
            toolbar.set_active_pane_item(active_item, window, cx);
        });
        self.search_bar.update(cx, |search_bar, cx| {
            search_bar.set_active_item(active_item, window, cx);
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
    pub fn handle_tab_drop(
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

// ═══ Action handler ═════════════════════════════════════════════

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

    fn handle_deploy_search(
        &mut self,
        _: &DeploySearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_bar.update(cx, |search_bar, cx| {
            search_bar.deploy(window, cx);
        });
    }

    fn handle_toggle_preview(
        &mut self,
        _: &TogglePreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_preview(window, cx);
    }
}

// ═══ Render ═════════════════════════════════════════════════════

impl Render for Pane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active_item_id = self.active;
        let transient_item_id = self.transient_item_id;
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
            .on_action(cx.listener(Self::handle_toggle_preview))
            .on_action(cx.listener(Self::handle_deploy_search))
            .child(render_tab_bar(
                &self.tabs,
                active_item_id,
                transient_item_id,
                pane_entity,
                &self.scroll_handle,
                cx,
            ))
            .child(self.toolbar.clone())
            .child(render_content(active_item_id, active_item, cx))
    }
}

// ═══ 私有渲染辅助函数 ═════════════════════════════════════════

// ── Tab Bar ──────────────────────────────────────────────────────────

/// 标签栏：一组标签的容器 + 末尾放置目标。
fn render_tab_bar(
    tabs: &[Box<dyn ItemHandle>],
    active_item_id: Option<EntityId>,
    transient_item_id: Option<EntityId>,
    pane_entity: gpui::Entity<Pane>,
    scroll_handle: &ScrollHandle,
    cx: &App,
) -> impl gpui::IntoElement {
    let children: Vec<AnyElement> = tabs
        .iter()
        .enumerate()
        .map(|(ix, item)| {
            render_tab(
                item.as_ref(),
                ix,
                Some(item.item_id()) == active_item_id,
                Some(item.item_id()) == transient_item_id,
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
    is_transient: bool,
    pane_entity: &gpui::Entity<Pane>,
    cx: &App,
) -> impl gpui::IntoElement {
    let item_id = item.item_id();
    let activate_entity = pane_entity.clone();
    let close_entity = pane_entity.clone();

    Tab::new(("tab", item_id))
        .selected(is_active)
        .italic(is_transient)
        .start_slot(item_icon(Some(item), cx))
        .end_slot(tab_end_glyph(
            &close_entity,
            item_id,
            item.is_dirty(cx),
            is_preview_item(item, cx),
            cx,
        ))
        .child(item.tab_content_text(0, cx))
        .group(TAB_HOVER_GROUP)
        .on_mouse_down(gpui::MouseButton::Left, move |event, window, cx| {
            let focus = activate_entity.update(cx, |pane, cx| {
                pane.activate_tab(item_id, window, cx);
                if event.click_count >= 2 {
                    pane.promote_transient_tab(item_id, cx);
                }
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

fn item_icon(item: Option<&dyn ItemHandle>, cx: &App) -> impl gpui::IntoElement {
    let path = item.and_then(|item| item.file_path(cx));
    let icon = match path {
        Some(path) if path.is_dir() => FileIcons::get_folder_icon(false, &path),
        Some(path) => FileIcons::get_icon(&path),
        None => FileIcons::get_icon(Path::new("")),
    };
    SvgIcon::new(icon)
}

fn is_preview_item(item: &dyn ItemHandle, cx: &App) -> bool {
    item.as_preview_item(cx).is_some()
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
        .on_click(
            move |_: &gpui::ClickEvent, window: &mut gpui::Window, cx: &mut gpui::App| {
                let pane_focus = entity.read(cx).focus.clone();
                let focus = entity.update(cx, |pane, cx| {
                    pane.close_tab(item_id, window, cx);
                    pane.active_item().map(|item| item.item_focus_handle(cx))
                });
                window.focus(&focus.unwrap_or(pane_focus));
                window.refresh();
            },
        )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabEndState {
    Close,
    Dirty,
    Preview,
}

fn tab_end_state(is_dirty: bool, is_preview: bool) -> TabEndState {
    if is_dirty {
        TabEndState::Dirty
    } else if is_preview {
        TabEndState::Preview
    } else {
        TabEndState::Close
    }
}

/// 标签尾部状态槽：未保存优先显示圆点，预览其次显示眼睛；悬停后都切换为关闭按钮。
fn tab_end_glyph(
    pane_entity: &gpui::Entity<Pane>,
    item_id: EntityId,
    is_dirty: bool,
    is_preview: bool,
    cx: &App,
) -> AnyElement {
    let state = tab_end_state(is_dirty, is_preview);
    if state == TabEndState::Close {
        return close_glyph(pane_entity, item_id, cx).into_any_element();
    }

    let slot_size = typography::ui();
    let (id, icon, icon_color) = match state {
        TabEndState::Dirty => (
            ("tab-dirty", item_id),
            "icons/circle.svg",
            color::current(cx).icon_accent,
        ),
        TabEndState::Preview => (
            ("tab-preview", item_id),
            "icons/eye.svg",
            color::current(cx).icon_muted,
        ),
        TabEndState::Close => unreachable!(),
    };
    div()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .size(slot_size)
        .child(
            div()
                .group_hover(TAB_HOVER_GROUP, |style| style.opacity(0.0))
                .child(Glyph::icon(id, icon).color(icon_color)),
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
    use crate::{Item, PreviewItem, PreviewItemHandle, PreviewProvider};

    /// 辅助视图类型，仅用于测试中创建窗口。
    struct TestView;
    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[test]
    fn dirty_indicator_takes_priority_over_preview_indicator() {
        assert_eq!(tab_end_state(false, false), TabEndState::Close);
        assert_eq!(tab_end_state(false, true), TabEndState::Preview);
        assert_eq!(tab_end_state(true, false), TabEndState::Dirty);
        assert_eq!(tab_end_state(true, true), TabEndState::Dirty);
    }

    /// 辅助：用 add_window_view 提供 window 上下文，以源码 Item 打开文件。
    fn open_item_in_test(
        cx: &mut TestAppContext,
        pane: &Entity<Pane>,
        path: PathBuf,
        buffer: Entity<Buffer>,
        allow_transient: bool,
    ) {
        cx.add_window_view(|window, cx| {
            pane.update(cx, |p, cx| {
                let item = cx.new(|cx| TestSourceItem::new(buffer.clone(), path.clone(), cx));
                p.open_item(Box::new(item), allow_transient, window, cx);
            });
            TestView
        });
    }

    fn open_file_in_test(
        cx: &mut TestAppContext,
        pane: &Entity<Pane>,
        path: PathBuf,
        buffer: Entity<Buffer>,
    ) {
        open_item_in_test(cx, pane, path, buffer, false);
    }

    fn open_transient_file_in_test(
        cx: &mut TestAppContext,
        pane: &Entity<Pane>,
        path: PathBuf,
        buffer: Entity<Buffer>,
    ) {
        open_item_in_test(cx, pane, path, buffer, true);
    }

    fn toggle_preview_in_test(cx: &mut TestAppContext, pane: &Entity<Pane>) {
        cx.add_window_view(|window, cx| {
            pane.update(cx, |pane, cx| {
                pane.toggle_preview(window, cx);
            });
            TestView
        });
    }

    fn test_buffer(cx: &mut TestAppContext, text: impl Into<String>) -> Entity<Buffer> {
        cx.new(|_| {
            Buffer::scratch(text.into(), BufferConfig::default()).expect("应创建测试 Buffer")
        })
    }

    /// 测试专用的源码 Item：编辑时标记脏并发射 Edit 事件（Pane 依赖它提升临时标签）。
    struct TestSourceItem {
        buffer: Entity<Buffer>,
        path: PathBuf,
        dirty: bool,
        focus: gpui::FocusHandle,
    }

    #[derive(Clone, Copy)]
    enum TestEvent {
        Edited,
    }

    impl TestSourceItem {
        fn new(buffer: Entity<Buffer>, path: PathBuf, cx: &mut Context<Self>) -> Self {
            Self {
                buffer,
                path,
                dirty: false,
                focus: cx.focus_handle(),
            }
        }

        /// 模拟用户编辑：标记脏并发射编辑事件。
        fn set_text(&mut self, _text: &str, cx: &mut Context<Self>) {
            self.dirty = true;
            cx.emit(TestEvent::Edited);
            cx.notify();
        }
    }

    impl EventEmitter<TestEvent> for TestSourceItem {}

    impl gpui::Focusable for TestSourceItem {
        fn focus_handle(&self, _cx: &App) -> gpui::FocusHandle {
            self.focus.clone()
        }
    }

    impl Render for TestSourceItem {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    impl Item for TestSourceItem {
        type Event = TestEvent;

        fn tab_content_text(&self, _detail: usize, _cx: &App) -> gpui::SharedString {
            self.path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
                .into()
        }

        fn to_item_events(event: &Self::Event, emit: &mut dyn FnMut(ItemEvent)) {
            match event {
                TestEvent::Edited => emit(ItemEvent::Edit),
            }
        }

        fn is_dirty(&self, _cx: &App) -> bool {
            self.dirty
        }

        fn file_path(&self, _cx: &App) -> Option<PathBuf> {
            Some(self.path.clone())
        }

        fn buffer(&self, _cx: &App) -> Option<Entity<Buffer>> {
            Some(self.buffer.clone())
        }
    }

    /// 测试专用的假预览 Item：转发源码 Item 元数据，展示键为 Preview("fake")。
    struct FakePreviewItem {
        source_item: Box<dyn ItemHandle>,
        focus: gpui::FocusHandle,
    }

    impl EventEmitter<()> for FakePreviewItem {}

    impl gpui::Focusable for FakePreviewItem {
        fn focus_handle(&self, _cx: &App) -> gpui::FocusHandle {
            self.focus.clone()
        }
    }

    impl Render for FakePreviewItem {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().child("fake preview")
        }
    }

    impl Item for FakePreviewItem {
        type Event = ();

        fn tab_content_text(&self, _detail: usize, cx: &App) -> gpui::SharedString {
            self.source_item.tab_content_text(0, cx)
        }

        fn is_dirty(&self, cx: &App) -> bool {
            self.source_item.is_dirty(cx)
        }

        fn file_path(&self, cx: &App) -> Option<PathBuf> {
            self.source_item.file_path(cx)
        }

        fn breadcrumbs(&self, cx: &App) -> Option<(Vec<gpui::SharedString>, Option<gpui::Font>)> {
            self.source_item.breadcrumbs(cx)
        }

        fn rename_path(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
            self.source_item.rename_path(from, to, cx);
        }

        fn buffer(&self, cx: &App) -> Option<Entity<Buffer>> {
            self.source_item.buffer(cx)
        }

        fn as_preview_item(
            &self,
            self_handle: &Entity<Self>,
            _cx: &App,
        ) -> Option<Box<dyn PreviewItemHandle>> {
            Some(Box::new(self_handle.clone()))
        }
    }

    impl PreviewItem for FakePreviewItem {
        fn source_item(&self, _cx: &App) -> Option<Box<dyn ItemHandle>> {
            Some(self.source_item.boxed_clone())
        }
    }

    /// 测试专用的假预览 Provider：匹配 svg 扩展名，创建 [`FakePreviewItem`]。
    struct FakePreviewProvider;

    impl PreviewProvider for FakePreviewProvider {
        fn supports(&self, path: &Path, _cx: &App) -> bool {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension == "svg")
        }

        fn create(&self, document: PreviewDocument, cx: &mut App) -> Box<dyn ItemHandle> {
            let view = cx.new(|cx| FakePreviewItem {
                source_item: document.source_item,
                focus: cx.focus_handle(),
            });
            Box::new(view)
        }
    }

    fn init_previews(cx: &mut TestAppContext) {
        cx.update(|cx| crate::register(FakePreviewProvider, cx));
    }

    /// 断言辅助：当前活动标签是否预览视图。
    fn assert_active_is_preview(pane: &Pane, cx: &App, expected: bool) {
        assert_eq!(
            pane.active_item().map(|item| is_preview_item(item, cx)),
            Some(expected),
            "活动标签的预览状态应一致"
        );
    }

    #[gpui::test]
    fn pane_owns_file_path_and_item_backed_by_the_given_buffer(cx: &mut TestAppContext) {
        let buffer = test_buffer(cx, "真实编辑器");
        let pane = cx.new(Pane::new);
        open_file_in_test(cx, &pane, PathBuf::from("demo.txt"), buffer.clone());

        let item = cx.read_entity(&pane, |pane, cx| {
            pane.active_item()
                .unwrap()
                .act_as::<TestSourceItem>(cx)
                .unwrap()
        });
        cx.read_entity(&item, |item, cx| assert!(!item.is_dirty(cx)));
        cx.update_entity(&item, |item, cx| item.set_text("阶段七", cx));
        cx.read_entity(&item, |item, cx| assert!(item.is_dirty(cx)));

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
            assert!(
                pane.active_item()
                    .unwrap()
                    .act_as::<TestSourceItem>(cx)
                    .is_some()
            );
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
    fn transient_tab_is_replaced_and_permanent_open_promotes_it(cx: &mut TestAppContext) {
        let pane = cx.new(Pane::new);
        let permanent_buffer = test_buffer(cx, "固定");
        open_file_in_test(cx, &pane, PathBuf::from("permanent.txt"), permanent_buffer);
        let first_buffer = test_buffer(cx, "第一个临时标签");
        open_transient_file_in_test(cx, &pane, PathBuf::from("first.txt"), first_buffer);
        let second_buffer = test_buffer(cx, "第二个临时标签");
        open_transient_file_in_test(cx, &pane, PathBuf::from("second.txt"), second_buffer);

        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 2, "新临时标签应替换旧临时标签");
            assert_eq!(
                pane.tabs[1].file_path(cx).as_deref(),
                Some(Path::new("second.txt"))
            );
            assert_eq!(pane.transient_item_id, Some(pane.tabs[1].item_id()));
        });

        // 模拟双击的第二次打开：同一路径固定打开，临时标签应被提升而非重复创建。
        let duplicate_buffer = test_buffer(cx, "不会替换已有 buffer");
        open_file_in_test(cx, &pane, PathBuf::from("second.txt"), duplicate_buffer);
        cx.read_entity(&pane, |pane, _| {
            assert_eq!(pane.tabs.len(), 2);
            assert_eq!(pane.transient_item_id, None);
        });

        let third_buffer = test_buffer(cx, "第三个临时标签");
        open_transient_file_in_test(cx, &pane, PathBuf::from("third.txt"), third_buffer);
        cx.read_entity(&pane, |pane, _| assert_eq!(pane.tabs.len(), 3));
    }

    #[gpui::test]
    fn editing_transient_tab_promotes_it_before_next_transient_tab(cx: &mut TestAppContext) {
        let pane = cx.new(Pane::new);
        let edited_buffer = test_buffer(cx, "临时标签");
        open_transient_file_in_test(cx, &pane, PathBuf::from("edited.txt"), edited_buffer);
        let item = cx.read_entity(&pane, |pane, cx| {
            pane.active_item()
                .unwrap()
                .act_as::<TestSourceItem>(cx)
                .unwrap()
        });
        cx.update_entity(&item, |item, cx| item.set_text("已修改", cx));
        cx.run_until_parked();
        cx.read_entity(&pane, |pane, _| assert_eq!(pane.transient_item_id, None));

        let next_buffer = test_buffer(cx, "下一项");
        open_transient_file_in_test(cx, &pane, PathBuf::from("next.txt"), next_buffer);
        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 2, "已编辑的原临时标签不应被替换");
            assert!(
                pane.tabs
                    .iter()
                    .any(|item| { item.file_path(cx).as_deref() == Some(Path::new("edited.txt")) })
            );
        });
    }

    #[gpui::test]
    fn single_click_previews_svg_but_double_click_file_opens_fixed_source(cx: &mut TestAppContext) {
        init_previews(cx);
        let pane = cx.new(Pane::new);
        let svg_buffer = test_buffer(
            cx,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><circle cx="8" cy="8" r="8"/></svg>"#,
        );
        open_transient_file_in_test(cx, &pane, PathBuf::from("icon.svg"), svg_buffer);

        cx.read_entity(&pane, |pane, cx| {
            let item_id = pane.active.unwrap();
            assert_eq!(pane.tabs.len(), 1);
            assert_eq!(pane.transient_item_id, Some(item_id));
            assert_active_is_preview(pane, cx, true);
            assert_eq!(
                pane.active_item().unwrap().tab_content_text(0, cx),
                "icon.svg"
            );
        });

        // 双击文件强制换成固定源码，而不是固定当前渲染内容。
        let duplicate_buffer = test_buffer(cx, "不会替换已有 SVG buffer");
        open_file_in_test(cx, &pane, PathBuf::from("icon.svg"), duplicate_buffer);
        cx.read_entity(&pane, |pane, _| {
            assert_eq!(pane.transient_item_id, None);
            assert_eq!(pane.tabs.len(), 1);
        });
        cx.read_entity(&pane, |pane, cx| {
            assert_active_is_preview(pane, cx, false);
        });
    }

    #[gpui::test]
    fn preview_toggle_replaces_content_and_preserves_transient_lifecycle(cx: &mut TestAppContext) {
        init_previews(cx);
        let pane = cx.new(Pane::new);
        let svg_buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
        open_transient_file_in_test(cx, &pane, PathBuf::from("toggle.svg"), svg_buffer);

        let transient_preview_id = cx.read_entity(&pane, |pane, _| pane.active.unwrap());
        toggle_preview_in_test(cx, &pane);
        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 1);
            assert_ne!(pane.tabs[0].item_id(), transient_preview_id);
            assert_eq!(pane.tabs[0].tab_content_text(0, cx), "toggle.svg");
            assert_eq!(pane.transient_item_id, Some(pane.tabs[0].item_id()));
            assert_eq!(pane.active, Some(pane.tabs[0].item_id()));
            assert_active_is_preview(pane, cx, false);
        });
    }

    #[gpui::test]
    fn only_transient_svg_files_open_in_preview_by_default(cx: &mut TestAppContext) {
        init_previews(cx);
        let permanent_pane = cx.new(Pane::new);
        let permanent_svg = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
        open_file_in_test(
            cx,
            &permanent_pane,
            PathBuf::from("permanent.svg"),
            permanent_svg,
        );
        cx.read_entity(&permanent_pane, |pane, cx| {
            assert_active_is_preview(pane, cx, false);
        });

        let text_pane = cx.new(Pane::new);
        let text_buffer = test_buffer(cx, "普通文本");
        open_transient_file_in_test(cx, &text_pane, PathBuf::from("preview.txt"), text_buffer);
        cx.read_entity(&text_pane, |pane, cx| {
            assert_active_is_preview(pane, cx, false);
        });
    }

    #[gpui::test]
    fn preview_is_unique_and_shares_the_source_buffer(cx: &mut TestAppContext) {
        init_previews(cx);
        let pane = cx.new(Pane::new);
        let svg_buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
        open_file_in_test(cx, &pane, PathBuf::from("shared.svg"), svg_buffer);
        let source_buffer = cx.read_entity(&pane, |pane, cx| {
            pane.active_item().unwrap().buffer(cx).unwrap()
        });
        toggle_preview_in_test(cx, &pane);
        toggle_preview_in_test(cx, &pane);
        toggle_preview_in_test(cx, &pane);

        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 1, "切换只替换当前标签的展示 Item");
            let active_buffer = pane.tabs[0].buffer(cx).unwrap();
            assert_eq!(source_buffer.entity_id(), active_buffer.entity_id());
            assert_active_is_preview(pane, cx, true);
        });
    }

    #[gpui::test]
    fn unsupported_file_does_not_open_a_preview_tab(cx: &mut TestAppContext) {
        init_previews(cx);
        let pane = cx.new(Pane::new);
        let text_buffer = test_buffer(cx, "普通文本");
        open_file_in_test(cx, &pane, PathBuf::from("plain.txt"), text_buffer);
        toggle_preview_in_test(cx, &pane);
        cx.read_entity(&pane, |pane, _| assert_eq!(pane.tabs.len(), 1));
    }

    #[gpui::test]
    fn closing_preview_closes_the_single_document_tab(cx: &mut TestAppContext) {
        init_previews(cx);
        let pane = cx.new(Pane::new);
        let svg_buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
        open_file_in_test(cx, &pane, PathBuf::from("close.svg"), svg_buffer);
        toggle_preview_in_test(cx, &pane);
        let preview_id = cx.read_entity(&pane, |pane, _| pane.active.unwrap());

        cx.add_window_view(|window, cx| {
            pane.update(cx, |pane, cx| pane.close_tab(preview_id, window, cx));
            TestView
        });
        cx.read_entity(&pane, |pane, _| {
            assert!(pane.tabs.is_empty());
            assert_eq!(pane.active, None);
        });
    }

    #[gpui::test]
    fn double_clicking_a_transient_tab_promotes_it(cx: &mut TestAppContext) {
        let pane = cx.new(Pane::new);
        let buffer = test_buffer(cx, "临时标签");
        open_transient_file_in_test(cx, &pane, PathBuf::from("preview.txt"), buffer);
        let item_id = cx.read_entity(&pane, |pane, _| pane.active.unwrap());

        cx.update_entity(&pane, |pane, cx| {
            pane.promote_transient_tab(item_id, cx);
        });
        cx.read_entity(&pane, |pane, _| {
            assert_eq!(pane.transient_item_id, None);
            assert_eq!(pane.tabs.len(), 1);
            assert_eq!(pane.active, Some(item_id));
        });
    }

    #[gpui::test]
    fn double_clicking_a_preview_tab_keeps_its_preview_content(cx: &mut TestAppContext) {
        init_previews(cx);
        let pane = cx.new(Pane::new);
        let buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
        open_transient_file_in_test(cx, &pane, PathBuf::from("previewed.svg"), buffer);
        let item_id = cx.read_entity(&pane, |pane, _| pane.active.unwrap());

        cx.update_entity(&pane, |pane, cx| {
            pane.promote_transient_tab(item_id, cx);
        });
        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.transient_item_id, None);
            assert_eq!(pane.active, Some(item_id));
            assert_active_is_preview(pane, cx, true);
        });
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
            assert_eq!(pane.tabs[0].tab_content_text(0, cx).as_ref(), "file0.txt");
            assert_eq!(pane.tabs[1].tab_content_text(0, cx).as_ref(), "file1.txt");
            assert_eq!(pane.tabs[2].tab_content_text(0, cx).as_ref(), "file2.txt");
            assert_eq!(pane.tabs[3].tab_content_text(0, cx).as_ref(), "file3.txt");
        });

        // 移动：将索引 2 移到索引 0
        cx.update_entity(&pane, |pane, _| pane.move_tab(2, 0));
        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 4);
            assert_eq!(pane.tabs[0].tab_content_text(0, cx).as_ref(), "file2.txt");
            assert_eq!(pane.tabs[1].tab_content_text(0, cx).as_ref(), "file0.txt");
            assert_eq!(pane.tabs[2].tab_content_text(0, cx).as_ref(), "file1.txt");
            assert_eq!(pane.tabs[3].tab_content_text(0, cx).as_ref(), "file3.txt");
        });

        // 移动：将索引 0 移到索引 3（拖到末尾）
        cx.update_entity(&pane, |pane, _| pane.move_tab(0, 3));
        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 4);
            assert_eq!(pane.tabs[0].tab_content_text(0, cx).as_ref(), "file0.txt");
            assert_eq!(pane.tabs[1].tab_content_text(0, cx).as_ref(), "file1.txt");
            assert_eq!(pane.tabs[2].tab_content_text(0, cx).as_ref(), "file3.txt");
            assert_eq!(pane.tabs[3].tab_content_text(0, cx).as_ref(), "file2.txt");
        });

        // 移动：不动（自身）
        cx.update_entity(&pane, |pane, _| pane.move_tab(1, 1));
        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 4);
            assert_eq!(pane.tabs[0].tab_content_text(0, cx).as_ref(), "file0.txt");
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
            assert_eq!(pane.tabs[0].tab_content_text(0, cx).as_ref(), "solo.txt");
        });
        // 拖到末尾（to_ix 超出范围）— clamp 后不应闪退
        cx.update_entity(&single_pane, |pane, _| pane.move_tab(0, 1));
        cx.read_entity(&single_pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 1);
            assert_eq!(pane.tabs[0].tab_content_text(0, cx).as_ref(), "solo.txt");
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
