//! Pane —— 单个编辑区 Pane 的 Entity。
//!
//! 持有自己的 FocusHandle、tabs、激活状态。
//! 渲染标签栏和编辑器内容，处理键盘事件。
//! Pane 通过 [`ItemHandle`] trait 统操作标签页，不依赖具体视图类型。

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, Entity, EntityId, EventEmitter, FocusHandle, Render, ScrollHandle,
    Window, div, prelude::*, px,
};
use zcv_actions::{CloseTab, NextTab, PrevTab, TogglePreview};
use zcv_theme::{FileIcons, color, typography};
use zcv_ui::{Button, SvgIcon, Tab};

use crate::layout_state::{SerializedPane, SerializedPaneItem};
use crate::preview::{PreviewDocument, provider_for};
use crate::tab_bar::{TabBar, TabBarTrailing};
use crate::toolbar::Toolbar;
use crate::{ItemEvent, ItemHandle};

// ═══ Pane 事件 ════════════════════════════════════════════════════════

/// Pane 对外发出的标签页事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneEvent {
    /// 新标签页添加。
    AddItem { item_id: EntityId },
    /// 活动标签页切换。
    ActivateItem { item_id: EntityId },
    /// 标签页被关闭。
    RemovedItem { item_id: EntityId },
    /// 标签全部关闭后请求移除 Pane 自身；宿主据此关闭面板。
    Remove,
}

impl EventEmitter<PaneEvent> for Pane {}

const TAB_HOVER_GROUP: &str = "pane.tab";

// ═══ DraggedTab —— 拖拽载荷 + 幽灵视图 ═════════════════════════════

/// 拖拽过程中传递的数据，同时也是拖拽时跟随鼠标的幽灵视图。
///
/// 仅支持同 Pane 内拖拽（drop 目标绑定在当前 Pane 的标签容器上）。
/// `pane` 引用只用于幽灵视图读取标签数据，不参与 drop 的跨 Pane 判断（跨 Pane 拖拽暂不支持）。
#[derive(Clone)]
struct DraggedTab {
    pane: Entity<Pane>,
    item_id: EntityId,
    ix: usize,
    is_active: bool,
}

impl Render for DraggedTab {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, item) = self
            .pane
            .read(cx)
            .tabs
            .get(self.ix)
            .map(|item| (item.tab_content_text(cx), Some(item.boxed_clone())))
            .unwrap_or_default();
        let is_transient = self.pane.read(cx).is_transient_item(self.item_id);
        Tab::new("")
            .selected(self.is_active)
            .italic(is_transient)
            .start_slot(item_icon(item.as_deref(), cx))
            .end_slot(tab_end_button(
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
    focus: FocusHandle,
    tabs: Vec<Box<dyn ItemHandle>>,
    active: Option<EntityId>,
    /// 当前临时源码及其预览标签；固定打开或发生编辑时一并提升为固定标签。
    transient_source_item_id: Option<EntityId>,
    transient_preview_item_id: Option<EntityId>,
    toolbar: Entity<Toolbar>,
    scroll_handle: ScrollHandle,
    /// 面板注入的标签栏右侧插槽构建器；渲染时原样转发给 TabBar（插槽本体在 TabBar 组件内）。
    tab_bar_trailing: Option<TabBarTrailing>,
}

impl Pane {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let toolbar = cx.new(|_| Toolbar::new());
        Self {
            focus: cx.focus_handle(),
            tabs: Vec::new(),
            active: None,
            transient_source_item_id: None,
            transient_preview_item_id: None,
            toolbar,
            scroll_handle: ScrollHandle::new(),
            tab_bar_trailing: None,
        }
    }

    /// 设置标签栏右侧功能插槽构建器，渲染时转发给 TabBar 的尾部插槽（不随标签滚动）。
    pub fn set_tab_bar_trailing<F>(&mut self, build: F)
    where
        F: Fn(&App) -> AnyElement + 'static,
    {
        self.tab_bar_trailing = Some(Rc::new(build));
    }

    /// 序列化固定标签；临时源码与临时预览不持久化。
    pub(crate) fn serialized(&self, cx: &App) -> SerializedPane {
        let mut items = Vec::new();
        let mut active_item = None;
        for item in &self.tabs {
            if self.is_transient_item(item.item_id()) {
                continue;
            }
            let serialized_item = if let Some(preview) = item.as_preview_item(cx) {
                preview
                    .source_item(cx)
                    .and_then(|source| source.item_path(cx).map(SerializedPaneItem::Preview))
            } else {
                item.serialized_pane_item(cx)
            };
            if let Some(serialized_item) = serialized_item {
                if self.active == Some(item.item_id()) {
                    active_item = Some(items.len());
                }
                items.push(serialized_item);
            }
        }
        SerializedPane { items, active_item }
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
                        if pane.transient_source_item_id == Some(item_id) {
                            pane.transient_source_item_id = None;
                            pane.transient_preview_item_id = None;
                        }
                        // 固定标签同样需要立即重绘未保存标记。
                        cx.notify();
                    }
                    ItemEvent::PathChanged
                    | ItemEvent::UpdateTab
                    | ItemEvent::UpdateBreadcrumbs => cx.notify(),
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
        cx.emit(PaneEvent::AddItem { item_id });
        cx.emit(PaneEvent::ActivateItem { item_id });
        cx.notify();
        focus
    }

    /// 打开一个 Item；`allow_transient` 为 true 时创建可被下一个单击替换的临时标签。
    /// 支持预览的格式单击默认只创建并激活预览标签；切换到源码时再创建源码标签，双击固定源码。
    pub fn open_item(
        &mut self,
        item: Box<dyn ItemHandle>,
        allow_transient: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        if let Some(path) = item.item_path(cx) {
            if let Some(source_index) = self.source_index_for_path(&path, cx) {
                let source_id = self.tabs[source_index].item_id();
                if !allow_transient {
                    self.promote_transient_tab(source_id, cx);
                    return self.activate_item_at(source_index, window, cx);
                }
                if let Some(preview_index) = self.preview_index_for_source(source_id, cx) {
                    return self.activate_item_at(preview_index, window, cx);
                }
                let focus = self.activate_item_at(source_index, window, cx);
                return focus;
            }

            if allow_transient && let Some(preview_index) = self.preview_index_for_path(&path, cx) {
                return self.activate_item_at(preview_index, window, cx);
            }
        }

        let transient_index = self.take_replaceable_transient(window, cx);

        let source_id = item.item_id();
        let path = item.item_path(cx);
        let multi_buffer = item.multi_buffer(cx);
        let Some(path) = path else {
            let source_focus = self.add_boxed_item_at(item, transient_index, window, cx);
            if allow_transient {
                self.transient_source_item_id = Some(source_id);
                cx.notify();
            }
            return source_focus;
        };
        if !allow_transient {
            return self.add_boxed_item_at(item, transient_index, window, cx);
        }
        let Some(multi_buffer) = multi_buffer else {
            let source_focus = self.add_boxed_item_at(item, transient_index, window, cx);
            self.transient_source_item_id = Some(source_id);
            cx.notify();
            return source_focus;
        };
        let Some(provider) = provider_for(&path, cx) else {
            let source_focus = self.add_boxed_item_at(item, transient_index, window, cx);
            self.transient_source_item_id = Some(source_id);
            cx.notify();
            return source_focus;
        };
        let preview = provider.create(
            PreviewDocument {
                path,
                source_item: item,
                multi_buffer,
            },
            cx,
        );
        let preview_id = preview.item_id();
        let preview_focus = self.add_boxed_item_at(preview, transient_index, window, cx);
        self.transient_preview_item_id = Some(preview_id);
        cx.notify();
        preview_focus
    }

    /// 在源码与渲染表现之间切换；
    /// 两个展示 Item 独立保留各自的滚动与焦点状态。
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
        if is_preview_item(self.tabs[index].as_ref(), cx) {
            let focus = self.activate_source_for_preview(index, window, cx)?;
            window.focus(&focus, cx);
            window.refresh();
            return Some(focus);
        }

        let source_item = self.tabs[index].boxed_clone();
        if let Some(preview_index) = self.preview_index_for_source(item_id, cx) {
            let focus = self.activate_item_at(preview_index, window, cx);
            window.focus(&focus, cx);
            window.refresh();
            return Some(focus);
        }
        let path = source_item.item_path(cx)?;
        let provider = provider_for(&path, cx)?;
        let multi_buffer = source_item.multi_buffer(cx)?;
        let preview = provider.create(
            PreviewDocument {
                path,
                source_item,
                multi_buffer,
            },
            cx,
        );
        let preview_id = preview.item_id();
        let preview_focus = self.add_boxed_item_at(preview, Some(index + 1), window, cx);
        if self.transient_source_item_id == Some(item_id) {
            self.transient_preview_item_id = Some(preview_id);
        }
        let focus = preview_focus;
        window.focus(&focus, cx);
        window.refresh();
        Some(focus)
    }

    /// 将指定源码恢复为固定预览标签；仅供工作区布局恢复使用。
    pub(crate) fn open_persistent_preview(
        &mut self,
        source_item: Box<dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<FocusHandle> {
        let path = source_item.item_path(cx)?;
        let provider = provider_for(&path, cx)?;
        let multi_buffer = source_item.multi_buffer(cx)?;
        let preview = provider.create(
            PreviewDocument {
                path,
                source_item,
                multi_buffer,
            },
            cx,
        );
        Some(self.add_boxed_item_at(preview, None, window, cx))
    }

    fn activate_source_for_preview(
        &mut self,
        preview_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<FocusHandle> {
        let source = self.tabs[preview_index]
            .as_preview_item(cx)
            .and_then(|preview| preview.source_item(cx))?;
        let source_id = source.item_id();
        if let Some(source_index) = self
            .tabs
            .iter()
            .position(|item| item.item_id() == source_id)
        {
            return Some(self.activate_item_at(source_index, window, cx));
        }
        let was_transient =
            self.transient_preview_item_id == Some(self.tabs[preview_index].item_id());
        let source_focus = self.add_boxed_item_at(source, None, window, cx);
        if was_transient {
            self.transient_source_item_id = Some(source_id);
        }
        Some(source_focus)
    }

    /// 移除可安全替换的临时标签并返回其位置；脏标签会先提升为固定标签。
    fn take_replaceable_transient(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let source_id = self.transient_source_item_id.take();
        let preview_id = self.transient_preview_item_id.take();
        if source_id.is_none() && preview_id.is_none() {
            return None;
        }
        let source_index = source_id.and_then(|source_id| {
            self.tabs
                .iter()
                .position(|item| item.item_id() == source_id)
        });
        let preview_index = preview_id.and_then(|preview_id| {
            self.tabs
                .iter()
                .position(|item| item.item_id() == preview_id)
        });
        let insertion_index = match (source_index, preview_index) {
            (Some(source_index), Some(preview_index)) => source_index.min(preview_index),
            (Some(index), None) | (None, Some(index)) => index,
            (None, None) => {
                self.transient_source_item_id = source_id;
                self.transient_preview_item_id = preview_id;
                return None;
            }
        };
        let dirty_item_id = source_id.or(preview_id).expect("临时标签至少包含一项");
        if self
            .tabs
            .iter()
            .find(|item| item.item_id() == dirty_item_id)
            .is_some_and(|item| item.is_dirty(cx))
        {
            self.transient_source_item_id = source_id;
            self.transient_preview_item_id = preview_id;
            return None;
        }
        if let Some(preview_id) = preview_id {
            self.close_tab(preview_id, window, cx);
        }
        if let Some(source_id) = source_id {
            self.close_tab(source_id, window, cx);
        }
        Some(insertion_index)
    }

    /// 将临时源码及其预览标签一并固定；内容视图状态保持不变。
    fn promote_transient_tab(&mut self, item_id: EntityId, cx: &mut Context<Self>) {
        if self.is_transient_item(item_id) {
            self.transient_source_item_id = None;
            self.transient_preview_item_id = None;
            cx.notify();
        }
    }

    fn activate_item_at(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        let item_id = self.tabs[index].item_id();
        let focus = self.tabs[index].item_focus_handle(cx);
        self.active = Some(item_id);
        self.scroll_to_tab(index);
        self.update_toolbar(window, cx);
        cx.emit(PaneEvent::ActivateItem { item_id });
        cx.notify();
        focus
    }

    fn source_index_for_path(&self, path: &Path, cx: &App) -> Option<usize> {
        self.tabs.iter().position(|item| {
            !is_preview_item(item.as_ref(), cx) && item.item_path(cx).as_deref() == Some(path)
        })
    }

    fn preview_index_for_path(&self, path: &Path, cx: &App) -> Option<usize> {
        self.tabs.iter().position(|item| {
            is_preview_item(item.as_ref(), cx) && item.item_path(cx).as_deref() == Some(path)
        })
    }

    fn preview_index_for_source(&self, source_id: EntityId, cx: &App) -> Option<usize> {
        self.tabs.iter().position(|item| {
            item.as_preview_item(cx)
                .and_then(|preview| preview.source_item(cx))
                .is_some_and(|source| source.item_id() == source_id)
        })
    }

    fn is_transient_item(&self, item_id: EntityId) -> bool {
        self.transient_source_item_id == Some(item_id)
            || self.transient_preview_item_id == Some(item_id)
    }

    /// 当前打开的所有标签（供宿主按具体 Item 类型操作）。
    pub fn tabs(&self) -> &[Box<dyn ItemHandle>] {
        &self.tabs
    }

    /// 焦点句柄。
    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
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
                let open_path = item.item_path(cx)?;
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
        let Some(pos) = self.tabs.iter().position(|item| item.item_id() == item_id) else {
            return;
        };
        // 关闭前记录焦点归属：仅当 Pane 或其 item 持有焦点时归还，避免抢占别处焦点（恢复会话、后台清理等非用户路径关闭时不触发）。
        let had_focus = self.has_focus(window, cx);
        if self.transient_source_item_id == Some(item_id) {
            self.transient_source_item_id = None;
        }
        if self.transient_preview_item_id == Some(item_id) {
            self.transient_preview_item_id = None;
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
        // 焦点归还：聚焦新激活 item；全部关闭后回落到 Pane 自身句柄（tab 栏容器 track_focus 挂载，焦点链与 Pane 快捷键保持有效）。
        if had_focus {
            let focus = self
                .active_item()
                .map(|item| item.item_focus_handle(cx))
                .unwrap_or_else(|| self.focus.clone());
            window.focus(&focus, cx);
        }
        cx.emit(PaneEvent::RemovedItem { item_id });
        // 空 Pane 请求移除自身。
        if self.tabs.is_empty() {
            cx.emit(PaneEvent::Remove);
        }
        cx.notify();
    }

    /// Pane 自身或其当前 item 是否持有焦点（决定关闭后是否归还焦点）。
    fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx)
            || self
                .active_item()
                .is_some_and(|item| item.item_focus_handle(cx).contains_focused(window, cx))
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
        self.active_item()?.active_path(cx)
    }

    fn focus_active_item(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.active_item() {
            window.focus(&item.item_focus_handle(cx), cx);
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
    fn handle_tab_drop(
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
        cx.emit(PaneEvent::ActivateItem {
            item_id: self.active.unwrap_or(dragged.item_id),
        });
        cx.notify();
    }
}

// ═══ Action handler ═════════════════════════════════════════════

impl Pane {
    /// 关闭活动标签；焦点归还在 `close_tab` 内统一处理（快捷键在 Pane 上下文触发，焦点必在 Pane 内）。
    fn handle_close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item_id) = self.active else {
            return;
        };
        self.close_tab(item_id, window, cx);
        window.refresh();
    }

    fn handle_next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.next_tab();
        // 关闭最后一个 tab 后按快捷键会走到这里：next_tab 对空 tabs 早退，active 可能为 None。
        let Some(item_id) = self.active else {
            return;
        };
        self.update_toolbar(window, cx);
        self.focus_active_item(window, cx);
        cx.emit(PaneEvent::ActivateItem { item_id });
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
        cx.emit(PaneEvent::ActivateItem { item_id });
        cx.notify();
        window.refresh();
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
        let transient_source_item_id = self.transient_source_item_id;
        let transient_preview_item_id = self.transient_preview_item_id;
        let active_item = self.active_item();
        let pane_entity = cx.entity();
        let trailing = self.tab_bar_trailing.clone();

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
            .on_action(cx.listener(Self::handle_close_tab))
            .on_action(cx.listener(Self::handle_next_tab))
            .on_action(cx.listener(Self::handle_prev_tab))
            .on_action(cx.listener(Self::handle_toggle_preview))
            .when(active_item_id.is_some(), |pane| {
                pane.child(render_tab_bar(
                    TabBarRenderParams {
                        tabs: &self.tabs,
                        active_item_id,
                        transient_source_item_id,
                        transient_preview_item_id,
                        pane_entity,
                        scroll_handle: &self.scroll_handle,
                        trailing,
                    },
                    cx,
                ))
            })
            .child(self.toolbar.clone())
            .child(render_content(active_item_id, active_item, cx))
    }
}

// ═══ 私有渲染辅助函数 ═════════════════════════════════════════

// ── Tab Bar ──────────────────────────────────────────────────────────

/// 标签栏：一组标签的容器 + 末尾放置目标 + 右侧功能插槽。
struct TabBarRenderParams<'a> {
    tabs: &'a [Box<dyn ItemHandle>],
    active_item_id: Option<EntityId>,
    transient_source_item_id: Option<EntityId>,
    transient_preview_item_id: Option<EntityId>,
    pane_entity: Entity<Pane>,
    scroll_handle: &'a ScrollHandle,
    trailing: Option<TabBarTrailing>,
}

fn render_tab_bar(params: TabBarRenderParams<'_>, cx: &App) -> impl gpui::IntoElement {
    let children: Vec<AnyElement> = params
        .tabs
        .iter()
        .enumerate()
        .map(|(ix, item)| {
            render_tab(
                item.as_ref(),
                ix,
                Some(item.item_id()) == params.active_item_id,
                Some(item.item_id()) == params.transient_source_item_id
                    || Some(item.item_id()) == params.transient_preview_item_id,
                &params.pane_entity,
                cx,
            )
            .into_any_element()
        })
        .chain(std::iter::once(
            render_tab_bar_drop_target(&params.pane_entity, params.tabs.len(), cx)
                .into_any_element(),
        ))
        .collect();

    let handle = params.scroll_handle.clone();
    let mut tab_bar = TabBar::new().track_scroll(params.scroll_handle);
    if let Some(trailing) = params.trailing {
        tab_bar = tab_bar.with_trailing(trailing);
    }

    let tab_bar = tab_bar.with_bar(
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
        .debug_selector(|| "tab-bar-area".into())
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
                let max_x = handle.max_offset().x;
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
        .end_slot(tab_end_button(
            &close_entity,
            item_id,
            item.is_dirty(cx),
            is_preview_item(item, cx),
            cx,
        ))
        .child(item.tab_content_text(cx))
        .group(TAB_HOVER_GROUP)
        .on_mouse_down(gpui::MouseButton::Left, move |event, window, cx| {
            let focus = activate_entity.update(cx, |pane, cx| {
                pane.activate_tab(item_id, window, cx);
                if event.click_count >= 2 {
                    pane.promote_transient_tab(item_id, cx);
                }
                cx.emit(PaneEvent::ActivateItem { item_id });
                cx.notify();
                pane.active_item().map(|item| item.item_focus_handle(cx))
            });
            if let Some(focus) = focus {
                window.focus(&focus, cx);
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
        .flex_grow(1.0)
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
    // Item 自定义图标优先（终端等无文件路径的 Item 提供自己的图标）。
    if let Some(icon) = item.and_then(|item| item.tab_icon(cx)) {
        return SvgIcon::new(icon);
    }
    let path = item.and_then(|item| item.item_path(cx));
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

/// 标签关闭按钮。
fn close_button(
    pane_entity: &gpui::Entity<Pane>,
    item_id: EntityId,
    cx: &App,
) -> impl gpui::IntoElement {
    let entity = pane_entity.clone();
    Button::icon(("tab-close", item_id), "icons/close.svg")
        .no_occlude()
        .label("关闭")
        .shortcut(&CloseTab, cx)
        .on_click(
            move |_: &gpui::ClickEvent, window: &mut gpui::Window, cx: &mut gpui::App| {
                // 焦点归还在 close_tab 内统一处理（点击时焦点在 Pane 内）。
                entity.update(cx, |pane, cx| pane.close_tab(item_id, window, cx));
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

/// 标签尾部状态槽：未保存优先显示圆点，预览其次显示眼睛；悬停后都切换为关闭按钮，无脏无预览时关闭按钮常显。
///
/// 三种状态共用同一槽结构（图标位 + 关闭按钮位），高度恒为图标尺寸：
/// 状态切换（圆点/眼睛 ↔ 关闭叉）只改透明度，不改变槽位高度，tab 高度稳定不抖动。
fn tab_end_button(
    pane_entity: &gpui::Entity<Pane>,
    item_id: EntityId,
    is_dirty: bool,
    is_preview: bool,
    cx: &App,
) -> AnyElement {
    let state = tab_end_state(is_dirty, is_preview);
    let is_close = state == TabEndState::Close;
    let indicator = match state {
        TabEndState::Close => None,
        TabEndState::Dirty => Some((
            ("tab-dirty", item_id),
            "icons/circle.svg",
            color::current(cx).icon_accent,
        )),
        TabEndState::Preview => Some((
            ("tab-preview", item_id),
            "icons/eye.svg",
            color::current(cx).icon_muted,
        )),
    };
    div()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .child(
            // 图标位：Close 态用透明占位保持槽高；脏/预览态 hover 时让位给关闭按钮。
            div()
                .opacity(if is_close { 0.0 } else { 1.0 })
                .group_hover(TAB_HOVER_GROUP, move |style| {
                    if is_close { style } else { style.opacity(0.0) }
                })
                .child(
                    indicator
                        .map(|(id, icon, icon_color)| {
                            SvgIcon::new(icon)
                                .id(id)
                                .color(icon_color)
                                .into_any_element()
                        })
                        .unwrap_or_else(|| div().size(typography::ui_size()).into_any_element()),
                ),
        )
        .child(
            // 关闭按钮位：Close 态常显；脏/预览态 hover 时浮现。
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .opacity(if is_close { 1.0 } else { 0.0 })
                .group_hover(TAB_HOVER_GROUP, move |style| {
                    if is_close { style } else { style.opacity(1.0) }
                })
                .child(close_button(pane_entity, item_id, cx)),
        )
        .into_any_element()
}

// ── Editor Content ────────────────────────────────────────────────────

/// 渲染 Pane 内容区；没有活动项时保持空白。
fn render_content(
    active_item_id: Option<EntityId>,
    active_item: Option<&dyn ItemHandle>,
    cx: &App,
) -> impl gpui::IntoElement {
    if active_item_id.is_none() {
        return div().flex_1().into_any_element();
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

    use gpui::{
        AppContext, Context, Pixels, Point, Render, TestAppContext, Window, div, prelude::*,
    };
    use zcv_language::LanguageBuffer;
    use zcv_multi_buffer::MultiBuffer;
    use zcv_text::{Buffer, BufferConfig};

    use super::*;
    use crate::{Item, PreviewItem, PreviewItemHandle, PreviewProvider, register};

    /// 辅助视图类型，仅用于测试中创建窗口。
    struct TestView;
    impl Render for TestView {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    struct TestToolbarItem {
        focus: FocusHandle,
    }

    impl EventEmitter<crate::ToolbarItemEvent> for TestToolbarItem {}

    impl Render for TestToolbarItem {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().track_focus(&self.focus)
        }
    }

    impl crate::ToolbarItemView for TestToolbarItem {
        fn set_active_pane_item(
            &mut self,
            _: Option<&dyn ItemHandle>,
            _: &mut Window,
            _: &mut Context<Self>,
        ) -> crate::ToolbarItemLocation {
            crate::ToolbarItemLocation::Secondary
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
        multi_buffer: Entity<MultiBuffer>,
        allow_transient: bool,
    ) {
        cx.add_window_view(|window, cx| {
            pane.update(cx, |p, cx| {
                let item = cx.new(|cx| TestSourceItem::new(multi_buffer.clone(), path.clone(), cx));
                p.open_item(Box::new(item), allow_transient, window, cx);
            });
            TestView
        });
    }

    fn open_file_in_test(
        cx: &mut TestAppContext,
        pane: &Entity<Pane>,
        path: PathBuf,
        multi_buffer: Entity<MultiBuffer>,
    ) {
        open_item_in_test(cx, pane, path, multi_buffer, false);
    }

    fn open_transient_file_in_test(
        cx: &mut TestAppContext,
        pane: &Entity<Pane>,
        path: PathBuf,
        multi_buffer: Entity<MultiBuffer>,
    ) {
        open_item_in_test(cx, pane, path, multi_buffer, true);
    }

    fn toggle_preview_in_test(cx: &mut TestAppContext, pane: &Entity<Pane>) {
        cx.add_window_view(|window, cx| {
            pane.update(cx, |pane, cx| {
                pane.toggle_preview(window, cx);
            });
            TestView
        });
    }

    fn test_buffer(cx: &mut TestAppContext, text: impl Into<String>) -> Entity<MultiBuffer> {
        let buffer = cx.new(|_| {
            Buffer::scratch(text.into(), BufferConfig::default()).expect("应创建测试 Buffer")
        });
        let language_buffer = cx.new(|cx| LanguageBuffer::new(buffer, None, cx));
        cx.new(|cx| MultiBuffer::from_working_source(language_buffer, cx))
    }

    /// 测试专用的源码 Item：编辑时标记脏并发射 Edit 事件（Pane 依赖它提升临时标签）。
    struct TestSourceItem {
        multi_buffer: Entity<MultiBuffer>,
        path: PathBuf,
        dirty: bool,
        focus: gpui::FocusHandle,
    }

    #[derive(Clone, Copy)]
    enum TestEvent {
        Edited,
    }

    impl TestSourceItem {
        fn new(multi_buffer: Entity<MultiBuffer>, path: PathBuf, cx: &mut Context<Self>) -> Self {
            Self {
                multi_buffer,
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

        fn tab_content_text(&self, _cx: &App) -> gpui::SharedString {
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

        fn item_path(&self, _cx: &App) -> Option<PathBuf> {
            Some(self.path.clone())
        }

        fn multi_buffer(&self, _cx: &App) -> Option<Entity<MultiBuffer>> {
            Some(self.multi_buffer.clone())
        }
    }

    struct TestCompositeItem {
        focus: FocusHandle,
    }

    impl EventEmitter<TestEvent> for TestCompositeItem {}

    impl gpui::Focusable for TestCompositeItem {
        fn focus_handle(&self, _cx: &App) -> FocusHandle {
            self.focus.clone()
        }
    }

    impl Render for TestCompositeItem {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    impl Item for TestCompositeItem {
        type Event = TestEvent;

        fn tab_content_text(&self, _cx: &App) -> gpui::SharedString {
            "组合文档".into()
        }

        fn serialized_pane_item(&self, _cx: &App) -> Option<SerializedPaneItem> {
            Some(SerializedPaneItem::Custom {
                kind: "test-composite".into(),
                state: serde_json::json!({ "group": "staged" }),
            })
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

        fn tab_content_text(&self, cx: &App) -> gpui::SharedString {
            self.source_item.tab_content_text(cx)
        }

        fn is_dirty(&self, cx: &App) -> bool {
            self.source_item.is_dirty(cx)
        }

        fn item_path(&self, cx: &App) -> Option<PathBuf> {
            self.source_item.item_path(cx)
        }

        fn breadcrumbs(
            &self,
            project_root: Option<&Path>,
            cx: &App,
        ) -> Option<(Vec<gpui::SharedString>, Option<gpui::Font>)> {
            self.source_item.breadcrumbs(project_root, cx)
        }

        fn rename_path(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
            self.source_item.rename_path(from, to, cx);
        }

        fn multi_buffer(&self, cx: &App) -> Option<Entity<MultiBuffer>> {
            self.source_item.multi_buffer(cx)
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
        cx.update(|cx| register(FakePreviewProvider, cx));
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
                    .item_path(cx)
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
                pane.tabs[1].item_path(cx).as_deref(),
                Some(Path::new("second.txt"))
            );
            assert_eq!(pane.transient_source_item_id, Some(pane.tabs[1].item_id()));
        });

        // 模拟双击的第二次打开：同一路径固定打开，临时标签应被提升而非重复创建。
        let duplicate_buffer = test_buffer(cx, "不会替换已有 buffer");
        open_file_in_test(cx, &pane, PathBuf::from("second.txt"), duplicate_buffer);
        cx.read_entity(&pane, |pane, _| {
            assert_eq!(pane.tabs.len(), 2);
            assert_eq!(pane.transient_source_item_id, None);
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
        cx.read_entity(&pane, |pane, _| {
            assert_eq!(pane.transient_source_item_id, None)
        });

        let next_buffer = test_buffer(cx, "下一项");
        open_transient_file_in_test(cx, &pane, PathBuf::from("next.txt"), next_buffer);
        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 2, "已编辑的原临时标签不应被替换");
            assert!(
                pane.tabs
                    .iter()
                    .any(|item| { item.item_path(cx).as_deref() == Some(Path::new("edited.txt")) })
            );
        });
    }

    #[gpui::test]
    fn single_click_opens_preview_but_double_click_replaces_it_with_source(
        cx: &mut TestAppContext,
    ) {
        init_previews(cx);
        let pane = cx.new(Pane::new);
        let svg_buffer = test_buffer(
            cx,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><circle cx="8" cy="8" r="8"/></svg>"#,
        );
        open_transient_file_in_test(cx, &pane, PathBuf::from("icon.svg"), svg_buffer);

        cx.read_entity(&pane, |pane, cx| {
            let preview_id = pane.active.unwrap();
            assert_eq!(pane.tabs.len(), 1);
            assert_eq!(pane.transient_source_item_id, None);
            assert_eq!(pane.transient_preview_item_id, Some(preview_id));
            assert_active_is_preview(pane, cx, true);
            assert_eq!(pane.active_item().unwrap().tab_content_text(cx), "icon.svg");
        });

        // 双击文件关闭临时预览，并在其位置打开固定源码。
        let source_buffer = test_buffer(cx, "固定源码");
        open_file_in_test(cx, &pane, PathBuf::from("icon.svg"), source_buffer);
        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.transient_source_item_id, None);
            assert_eq!(pane.transient_preview_item_id, None);
            assert_eq!(pane.tabs.len(), 1);
            assert_active_is_preview(pane, cx, false);
        });
    }

    #[gpui::test]
    fn opening_another_transient_preview_replaces_the_previous_preview(cx: &mut TestAppContext) {
        init_previews(cx);
        let pane = cx.new(Pane::new);
        let first_buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
        open_transient_file_in_test(cx, &pane, PathBuf::from("first.svg"), first_buffer);
        let second_buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
        open_transient_file_in_test(cx, &pane, PathBuf::from("second.svg"), second_buffer);

        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 1);
            assert!(
                pane.tabs
                    .iter()
                    .all(|item| { item.item_path(cx).as_deref() == Some(Path::new("second.svg")) })
            );
        });
    }

    #[gpui::test]
    fn preview_toggle_switches_tabs_and_preserves_transient_lifecycle(cx: &mut TestAppContext) {
        init_previews(cx);
        let pane = cx.new(Pane::new);
        let svg_buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
        open_transient_file_in_test(cx, &pane, PathBuf::from("toggle.svg"), svg_buffer);

        let preview_id = cx.read_entity(&pane, |pane, _| {
            let preview_id = pane.active.unwrap();
            assert_eq!(pane.tabs.len(), 1);
            assert_eq!(pane.transient_source_item_id, None);
            assert_eq!(pane.transient_preview_item_id, Some(preview_id));
            preview_id
        });
        let source_id = cx.read_entity(&pane, |pane, cx| {
            pane.tabs[0]
                .as_preview_item(cx)
                .and_then(|preview| preview.source_item(cx))
                .unwrap()
                .item_id()
        });
        toggle_preview_in_test(cx, &pane);
        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 2);
            assert_eq!(pane.transient_source_item_id, Some(source_id));
            assert_eq!(pane.transient_preview_item_id, Some(preview_id));
            assert_eq!(pane.active, Some(source_id));
            assert!(is_preview_item(pane.tabs[0].as_ref(), cx));
            assert!(!is_preview_item(pane.tabs[1].as_ref(), cx));
            assert_active_is_preview(pane, cx, false);
        });
        toggle_preview_in_test(cx, &pane);
        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 2);
            assert_eq!(pane.active, Some(preview_id));
            assert_active_is_preview(pane, cx, true);
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
    fn preview_is_unique_and_shares_the_source_document(cx: &mut TestAppContext) {
        init_previews(cx);
        let pane = cx.new(Pane::new);
        let svg_buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
        open_file_in_test(cx, &pane, PathBuf::from("shared.svg"), svg_buffer);
        let source_document = cx.read_entity(&pane, |pane, cx| {
            pane.active_item().unwrap().multi_buffer(cx).unwrap()
        });
        toggle_preview_in_test(cx, &pane);
        toggle_preview_in_test(cx, &pane);
        toggle_preview_in_test(cx, &pane);

        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 2, "源码和预览应分别保留为两个标签");
            let active_document = pane.active_item().unwrap().multi_buffer(cx).unwrap();
            assert_eq!(source_document.entity_id(), active_document.entity_id());
            assert_active_is_preview(pane, cx, true);
        });
    }

    #[gpui::test]
    fn serialized_pane_keeps_fixed_preview_and_its_active_position(cx: &mut TestAppContext) {
        init_previews(cx);
        let pane = cx.new(Pane::new);
        let buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
        open_file_in_test(cx, &pane, PathBuf::from("persisted.svg"), buffer);
        toggle_preview_in_test(cx, &pane);

        cx.read_entity(&pane, |pane, cx| {
            let state = pane.serialized(cx);
            assert_eq!(
                state.items,
                vec![
                    SerializedPaneItem::Source(PathBuf::from("persisted.svg")),
                    SerializedPaneItem::Preview(PathBuf::from("persisted.svg")),
                ]
            );
            assert_eq!(state.active_item, Some(1));
        });
    }

    #[gpui::test]
    fn serialized_pane_omits_transient_preview(cx: &mut TestAppContext) {
        init_previews(cx);
        let pane = cx.new(Pane::new);
        let buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
        open_transient_file_in_test(cx, &pane, PathBuf::from("transient.svg"), buffer);

        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.serialized(cx), SerializedPane::default());
        });
    }

    #[gpui::test]
    fn serialized_pane_keeps_fixed_custom_item(cx: &mut TestAppContext) {
        let pane = cx.new(Pane::new);
        cx.add_window_view(|window, cx| {
            pane.update(cx, |pane, cx| {
                let item = cx.new(|cx| TestCompositeItem {
                    focus: cx.focus_handle(),
                });
                pane.open_item(Box::new(item), false, window, cx);
            });
            TestView
        });

        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(
                pane.serialized(cx),
                SerializedPane {
                    items: vec![SerializedPaneItem::Custom {
                        kind: "test-composite".into(),
                        state: serde_json::json!({ "group": "staged" }),
                    }],
                    active_item: Some(0),
                }
            );
        });
    }

    #[gpui::test]
    fn restored_preview_is_fixed(cx: &mut TestAppContext) {
        init_previews(cx);
        let pane = cx.new(Pane::new);
        let buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);

        cx.add_window_view(|window, cx| {
            pane.update(cx, |pane, cx| {
                let source = cx.new(|cx| {
                    TestSourceItem::new(buffer.clone(), PathBuf::from("restored.svg"), cx)
                });
                pane.open_persistent_preview(Box::new(source), window, cx)
                    .expect("已注册的预览应能恢复");
            });
            TestView
        });

        cx.read_entity(&pane, |pane, cx| {
            let preview = pane.active_item().expect("预览应被加入标签栏");
            assert!(is_preview_item(preview, cx));
            assert!(!pane.is_transient_item(preview.item_id()));
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
    fn closing_preview_keeps_the_source_tab(cx: &mut TestAppContext) {
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
        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 1);
            assert!(!is_preview_item(pane.active_item().unwrap(), cx));
            assert!(pane.active.is_some());
        });
    }

    #[gpui::test]
    fn closing_source_keeps_its_preview(cx: &mut TestAppContext) {
        init_previews(cx);
        let pane = cx.new(Pane::new);
        let buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
        open_file_in_test(cx, &pane, PathBuf::from("close-source.svg"), buffer);
        toggle_preview_in_test(cx, &pane);
        let source_id = cx.read_entity(&pane, |pane, cx| {
            pane.tabs
                .iter()
                .find(|item| !is_preview_item(item.as_ref(), cx))
                .unwrap()
                .item_id()
        });

        cx.add_window_view(|window, cx| {
            pane.update(cx, |pane, cx| pane.close_tab(source_id, window, cx));
            TestView
        });
        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 1);
            assert!(is_preview_item(pane.active_item().unwrap(), cx));
            assert!(pane.active.is_some());
        });
    }

    #[gpui::test]
    fn showing_source_from_an_orphaned_preview_appends_a_tab(cx: &mut TestAppContext) {
        init_previews(cx);
        let pane = cx.new(Pane::new);
        let svg_buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
        open_file_in_test(cx, &pane, PathBuf::from("orphaned.svg"), svg_buffer);
        toggle_preview_in_test(cx, &pane);
        let source_id = cx.read_entity(&pane, |pane, cx| {
            pane.tabs
                .iter()
                .find(|item| !is_preview_item(item.as_ref(), cx))
                .unwrap()
                .item_id()
        });

        cx.add_window_view(|window, cx| {
            pane.update(cx, |pane, cx| pane.close_tab(source_id, window, cx));
            TestView
        });
        let text_buffer = test_buffer(cx, "另一个标签");
        open_file_in_test(cx, &pane, PathBuf::from("other.txt"), text_buffer);
        cx.add_window_view(|window, cx| {
            pane.update(cx, |pane, cx| {
                pane.activate_item_at(0, window, cx);
                pane.toggle_preview(window, cx);
            });
            TestView
        });

        cx.read_entity(&pane, |pane, cx| {
            assert_eq!(pane.tabs.len(), 3);
            assert!(is_preview_item(pane.tabs[0].as_ref(), cx));
            assert_eq!(
                pane.tabs[1].item_path(cx).as_deref(),
                Some(Path::new("other.txt"))
            );
            assert_eq!(
                pane.tabs[2].item_path(cx).as_deref(),
                Some(Path::new("orphaned.svg"))
            );
            assert_eq!(pane.active, Some(pane.tabs[2].item_id()));
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
            assert_eq!(pane.transient_source_item_id, None);
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
            assert_eq!(pane.transient_source_item_id, None);
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
                if matches!(event, PaneEvent::RemovedItem { .. }) {
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

    #[gpui::test]
    fn closing_active_tab_from_toolbar_focuses_next_tab(cx: &mut TestAppContext) {
        let (pane, cx) = cx.add_window_view(|_, cx| Pane::new(cx));
        let first_buffer = test_buffer(cx, "第一个标签");
        let second_buffer = test_buffer(cx, "项目搜索标签");
        let toolbar_item = cx.new(|cx| TestToolbarItem {
            focus: cx.focus_handle(),
        });

        cx.update(|window, cx| {
            let toolbar = pane.read(cx).toolbar().clone();
            toolbar.update(cx, |toolbar, cx| {
                toolbar.add_item(toolbar_item.clone(), window, cx);
            });

            pane.update(cx, |pane, cx| {
                let first = cx.new(|cx| {
                    TestSourceItem::new(first_buffer.clone(), PathBuf::from("first.txt"), cx)
                });
                pane.open_item(Box::new(first), false, window, cx);

                let second = cx.new(|cx| {
                    TestSourceItem::new(
                        second_buffer.clone(),
                        PathBuf::from("project-search.txt"),
                        cx,
                    )
                });
                pane.open_item(Box::new(second), false, window, cx);
            });
        });
        cx.run_until_parked();
        cx.refresh().expect("测试窗口应可刷新");

        let toolbar_focus = cx.read_entity(&toolbar_item, |item, _| item.focus.clone());
        cx.update(|window, cx| window.focus(&toolbar_focus, cx));
        cx.run_until_parked();

        cx.update(|window, cx| {
            assert!(
                pane.read(cx).focus.contains_focused(window, cx),
                "工具栏子项焦点应属于 Pane"
            );
            let item_id = pane.read(cx).active.expect("应有活动标签");
            pane.update(cx, |pane, cx| pane.close_tab(item_id, window, cx));

            let active_focus = pane
                .read(cx)
                .active_item()
                .expect("关闭后应激活另一个标签")
                .item_focus_handle(cx);
            assert!(
                active_focus.is_focused(window),
                "关闭工具栏所属标签后应将焦点交给新活动标签"
            );
        });
    }

    #[gpui::test]
    fn tab_bar_is_only_rendered_when_pane_has_an_active_item(cx: &mut TestAppContext) {
        let (pane, cx) = cx.add_window_view(|_, cx| Pane::new(cx));
        cx.run_until_parked();
        cx.refresh().expect("测试窗口应可刷新");
        assert!(
            cx.debug_bounds("tab-bar-area").is_none(),
            "空 Pane 不应残留 TabBar 下边框"
        );

        let buffer = test_buffer(cx, "内容");
        cx.update(|window, cx| {
            pane.update(cx, |pane, cx| {
                let item = cx.new(|cx| TestSourceItem::new(buffer, PathBuf::from("demo.txt"), cx));
                pane.open_item(Box::new(item), false, window, cx);
            });
        });
        cx.run_until_parked();
        cx.refresh().expect("测试窗口应可刷新");
        assert!(
            cx.debug_bounds("tab-bar-area").is_some(),
            "存在活动项时应恢复 TabBar"
        );
    }

    #[gpui::test]
    fn clicking_empty_pane_area_moves_focus_to_pane(cx: &mut TestAppContext) {
        // 决定性验证：点击空白 pane（无活动文件）后，焦点转移给 pane 自身
        // （gpui 对 track_focus 元素的"聚焦点击"内建行为：mouse down Bubble 阶段自动 window.focus）。
        // 这解释了"项目树/终端点击空白 pane 后失焦"——焦点去了 pane，行为正确；
        // 终端光标消失依赖 on_blur（焦点事件在下一帧绘制时分发），测试环境窗口不激活、
        // 焦点事件路径被清空，无法在单测中验证 on_blur 时序。
        let (pane, cx) = cx.add_window_view(|_, cx| Pane::new(cx));
        cx.run_until_parked();
        cx.refresh().expect("测试窗口应可刷新");

        // 用一个独立的焦点句柄模拟"外部组件（如终端）持有焦点"。
        let external_focus = cx.update(|_, app| app.focus_handle());
        cx.update(|window, cx| window.focus(&external_focus, cx));
        cx.run_until_parked();
        cx.update(|window, _| {
            assert!(external_focus.is_focused(window), "前置：外部句柄应已聚焦");
        });

        // 点击空白 pane 内容区（窗口中心）。
        let bounds = cx.update(|window, _| window.bounds());
        let click: Point<Pixels> = Point::new(bounds.size.width / 2.0, bounds.size.height / 2.0);
        cx.simulate_mouse_down(click, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.run_until_parked();

        cx.update(|window, cx| {
            assert!(
                !external_focus.is_focused(window),
                "点击空白 pane 转移了焦点"
            );
            assert!(
                pane.read(cx).focus.is_focused(window),
                "焦点应转移到 pane 自身（track_focus 的聚焦点击）"
            );
        });
    }
}
