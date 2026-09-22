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
use zcv_theme::{FileIcons, color};
use zcv_ui::{Button, SvgIcon, Tab};

use crate::layout_state::{SerializedPane, SerializedPaneItem};
use crate::preview::{OpenPathCallback, PreviewDocument, source_provider_for};
use crate::tab_bar::{TabBar, TabBarTrailing};
use crate::{ItemEvent, ItemHandle, Toolbar};

// ═══ Pane 事件 ════════════════════════════════════════════════════════

/// Pane 对外发出的标签页事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneEvent {
    /// 新标签页添加。
    AddItem {
        item_id: EntityId,
    },
    /// 活动标签页切换。
    ActivateItem {
        item_id: EntityId,
    },
    /// 标签页被关闭。
    RemovedItem {
        item_id: EntityId,
    },
    ItemError {
        message: String,
    },
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                window.rem_size(),
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
    scroll_handle: ScrollHandle,
    /// 面板注入的标签栏右侧插槽构建器；渲染时原样转发给 TabBar（插槽本体在 TabBar 组件内）。
    tab_bar_trailing: Option<TabBarTrailing>,
    /// 工作区注入的文件打开能力；独立测试 Pane 可以不提供该能力。
    open_path: Option<OpenPathCallback>,
    /// 顶部工具区；工具项由装配层注册，Pane 只负责随活动项同步。
    toolbar: Entity<Toolbar>,
}

impl Pane {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            tabs: Vec::new(),
            active: None,
            transient_source_item_id: None,
            transient_preview_item_id: None,
            scroll_handle: ScrollHandle::new(),
            tab_bar_trailing: None,
            open_path: None,
            toolbar: cx.new(|_| Toolbar::new()),
        }
    }

    /// 注入工作区的文件打开能力，供预览内容中的本地链接使用。
    pub fn set_open_path(&mut self, open_path: OpenPathCallback) {
        self.open_path = Some(open_path);
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
                    ItemEvent::Error(message) => cx.emit(PaneEvent::ItemError { message }),
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
        cx.emit(PaneEvent::AddItem { item_id });
        cx.emit(PaneEvent::ActivateItem { item_id });
        self.update_toolbar(window, cx);
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
        let Some(provider) = source_provider_for(&path, cx) else {
            let source_focus = self.add_boxed_item_at(item, transient_index, window, cx);
            self.transient_source_item_id = Some(source_id);
            cx.notify();
            return source_focus;
        };
        let preview = provider.create(
            PreviewDocument::Source {
                path,
                source_item: item,
                multi_buffer,
                open_path: self.open_path.clone(),
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
        let provider = source_provider_for(&path, cx)?;
        let multi_buffer = source_item.multi_buffer(cx)?;
        let preview = provider.create(
            PreviewDocument::Source {
                path,
                source_item,
                multi_buffer,
                open_path: self.open_path.clone(),
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
        let provider = source_provider_for(&path, cx)?;
        let multi_buffer = source_item.multi_buffer(cx)?;
        let preview = provider.create(
            PreviewDocument::Source {
                path,
                source_item,
                multi_buffer,
                open_path: self.open_path.clone(),
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
        cx.emit(PaneEvent::ActivateItem { item_id });
        self.update_toolbar(window, cx);
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

    /// 顶部工具区；装配层通过它注册工具项。
    pub fn toolbar(&self) -> &Entity<Toolbar> {
        &self.toolbar
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
            if self.active == Some(item_id) {
                return;
            }
            self.active = Some(item_id);
            self.scroll_to_tab(pos);
            cx.emit(PaneEvent::ActivateItem { item_id });
            self.update_toolbar(window, cx);
            cx.notify();
        }
    }

    /// 切换到下一个 tab，并滚入视图。
    fn next_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        self.update_toolbar(window, cx);
    }

    /// 切换到上一个 tab，并滚入视图。
    fn prev_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        self.update_toolbar(window, cx);
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
        self.tabs[pos].close(window, cx);
        self.tabs.remove(pos);
        if self.active == Some(item_id) {
            self.active = self
                .tabs
                .get(pos)
                .map(|item| item.item_id())
                .or_else(|| self.tabs.last().map(|item| item.item_id()));
        }
        self.update_toolbar(window, cx);
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

    /// 把当前活动 Item 同步给工具区，让每个工具项重新选择位置与显隐。
    fn update_toolbar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_item = self.active_item().map(ItemHandle::boxed_clone);
        self.toolbar.update(cx, |toolbar, cx| {
            toolbar.set_active_item(active_item.as_deref(), window, cx);
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
        self.update_toolbar(window, cx);
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
        self.next_tab(window, cx);
        // 关闭最后一个 tab 后按快捷键会走到这里：next_tab 对空 tabs 早退，active 可能为 None。
        let Some(item_id) = self.active else {
            return;
        };
        self.focus_active_item(window, cx);
        cx.emit(PaneEvent::ActivateItem { item_id });
        cx.notify();
        window.refresh();
    }

    fn handle_prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.prev_tab(window, cx);
        let Some(item_id) = self.active else {
            return;
        };
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    window,
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

fn render_tab_bar(
    params: TabBarRenderParams<'_>,
    window: &Window,
    cx: &App,
) -> impl gpui::IntoElement {
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
                window.rem_size(),
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
    ui_size: gpui::Pixels,
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
            ui_size,
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
        .shortcut(zcv_keymap::display_shortcut(&CloseTab, cx))
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
    ui_size: gpui::Pixels,
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
            // 指示器覆盖层：脏/预览态显示，hover 时让位给关闭按钮。
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
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
                        .unwrap_or_else(|| div().size(ui_size).into_any_element()),
                ),
        )
        .child(
            // 关闭按钮保持在正常布局流中，作为尾部槽位的尺寸来源。
            // Close 态常显；脏/预览态 hover 时浮现，但隐藏时仍保留占位。
            div()
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
#[path = "test/pane_tests.rs"]
mod tests;
