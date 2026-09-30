//! 语法大纲面板。
//!
//! 面板持有当前活动编辑器、筛选结果和折叠状态；
//! 语法数据由 `zcv-editor` 提供，行的通用树几何由 `zcv-ui` 提供。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, Context, Entity, FocusHandle, Render, Task, UniformListScrollHandle, Window, div,
    prelude::*, uniform_list,
};
use zcv_editor::{Editor, EditorEvent, OutlineVersion};
use zcv_language::{LanguageRegistry, OutlineItem};
use zcv_theme::{color, space};
use zcv_ui::{Scrollbar, SearchInput};
use zcv_workspace::{Pane, PaneEvent, Panel, PanelEvent};

mod outline_item;
use outline_item::OutlineItemKey;

/// 当前活动编辑器的大纲面板。
pub struct OutlinePanel {
    focus: FocusHandle,
    pane: Entity<Pane>,
    active_editor: Option<Entity<Editor>>,
    editor_subscription: Option<gpui::Subscription>,
    search_input: Entity<Editor>,
    _search_subscription: gpui::Subscription,
    _pane_subscription: gpui::Subscription,
    /// 当前查询筛选后的可见大纲项。
    items: Vec<OutlineItem>,
    /// 已安装版本上的未过滤大纲项；查询变化只在其上重筛。
    source_items: Vec<OutlineItem>,
    /// 已安装 source_items 对应的失效键；None 表示尚无有效大纲。
    outline_version: Option<OutlineVersion>,
    /// 防抖后的后台重算任务；替换或清空即取消旧任务。
    refresh_task: Option<Task<()>>,
    /// 面板是否可见且被 Dock 激活；不可见时不计算。
    active: bool,
    collapsed_items: HashSet<OutlineItemKey>,
    scroll_handle: UniformListScrollHandle,
    scrollbar: Scrollbar<UniformListScrollHandle>,
}

/// 组合文档持续变化时合并重算的防抖窗口。
const OUTLINE_REFRESH_DEBOUNCE: Duration = Duration::from_millis(50);

/// 是否需要重建大纲：面板可见，且当前版本与已安装版本不同。
///
/// 滚动、绘制与选择变化只唤醒订阅，不改变版本，因此这里稳定返回 false。
fn outline_refresh_needed(active: bool, version_changed: bool) -> bool {
    active && version_changed
}

/// 按大纲文本过滤；匹配不改变语法层结果的顺序和层级。
///
/// 查询已在调用方规范化为小写。
fn filter_outline_items(source: &[OutlineItem], query: &str) -> Vec<OutlineItem> {
    if query.is_empty() {
        return source.to_vec();
    }
    source
        .iter()
        .filter(|item| item.text.to_lowercase().contains(query))
        .cloned()
        .collect()
}

impl OutlinePanel {
    pub fn new(
        pane: Entity<Pane>,
        language_registry: Arc<LanguageRegistry>,
        cx: &mut Context<Self>,
    ) -> Self {
        let search_input = cx.new(move |cx| Editor::single_line(language_registry, cx));
        search_input.update(cx, |editor, cx| {
            editor.set_placeholder_text("筛选大纲…", cx);
        });
        let search_subscription =
            cx.subscribe(&search_input, |panel, _input, event: &EditorEvent, cx| {
                if matches!(event, EditorEvent::Edited { .. }) {
                    // 查询变化只重筛已缓存项，不重新查询语法层。
                    panel.apply_filter(cx);
                }
            });
        let pane_subscription = cx.subscribe(&pane, |panel, pane, event: &PaneEvent, cx| {
            if matches!(
                event,
                PaneEvent::AddItem { .. }
                    | PaneEvent::ActivateItem { .. }
                    | PaneEvent::RemovedItem { .. }
            ) {
                panel.refresh_active_editor(&pane, cx);
            }
        });
        let scroll_handle = UniformListScrollHandle::default();
        let mut panel = Self {
            focus: cx.focus_handle(),
            active_editor: None,
            editor_subscription: None,
            pane,
            search_input,
            _search_subscription: search_subscription,
            _pane_subscription: pane_subscription,
            items: Vec::new(),
            source_items: Vec::new(),
            outline_version: None,
            refresh_task: None,
            active: false,
            collapsed_items: HashSet::new(),
            scrollbar: Scrollbar::vertical(scroll_handle.clone()),
            scroll_handle,
        };
        let pane = panel.pane.clone();
        panel.refresh_active_editor(&pane, cx);
        panel
    }

    fn refresh_active_editor(&mut self, pane: &Entity<Pane>, cx: &mut Context<Self>) {
        let next_editor = pane
            .read(cx)
            .active_item()
            .and_then(|item| item.act_as::<Editor>(cx));
        let changed = self.active_editor.as_ref().map(Entity::entity_id)
            != next_editor.as_ref().map(Entity::entity_id);
        if changed {
            self.editor_subscription = next_editor.as_ref().map(|editor| {
                // 语义事件唤醒：只监听文档推进，滚动等纯重绘不发布这些事实。
                cx.subscribe(editor, |panel, _editor, event: &EditorEvent, cx| {
                    if matches!(event, EditorEvent::DocumentChanged) {
                        panel.invalidate_outline(cx);
                    }
                })
            });
            self.active_editor = next_editor;
            self.source_items.clear();
            self.outline_version = None;
            self.apply_filter(cx);
        }
        self.invalidate_outline(cx);
    }

    /// 订阅唤醒入口：面板可见且失效键变化时，启动一次防抖后台重算。
    fn invalidate_outline(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.active_editor.clone() else {
            if !self.source_items.is_empty() || self.outline_version.is_some() {
                self.source_items.clear();
                self.outline_version = None;
                self.apply_filter(cx);
            }
            return;
        };
        let version = editor.read(cx).outline_version(cx);
        let version_changed = self.outline_version != Some(version);
        if !outline_refresh_needed(self.active, version_changed) {
            return;
        }
        let source = editor.read(cx).outline_source(cx);
        let editor_id = editor.entity_id();
        self.refresh_task = Some(cx.spawn(async move |panel, cx| {
            cx.background_executor()
                .timer(OUTLINE_REFRESH_DEBOUNCE)
                .await;
            let items = cx
                .background_executor()
                .spawn(async move { source.items() })
                .await;
            panel
                .update(cx, |panel, cx| {
                    panel.install_source_items(editor_id, version, items, cx);
                })
                .ok();
        }));
    }

    /// 安装后台重算结果；编辑器已切换或版本已过期时丢弃。
    fn install_source_items(
        &mut self,
        editor_id: gpui::EntityId,
        version: OutlineVersion,
        items: Vec<OutlineItem>,
        cx: &mut Context<Self>,
    ) {
        let same_editor = self
            .active_editor
            .as_ref()
            .is_some_and(|editor| editor.entity_id() == editor_id);
        if !same_editor
            || self
                .active_editor
                .as_ref()
                .map(|editor| editor.read(cx).outline_version(cx))
                != Some(version)
        {
            return;
        }
        self.source_items = items;
        self.outline_version = Some(version);
        self.apply_filter(cx);
    }

    /// 按当前查询在缓存项上重筛并刷新折叠集合；不访问语法层。
    fn apply_filter(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).text(cx).trim().to_lowercase();
        let items = filter_outline_items(&self.source_items, &query);
        let current_keys: HashSet<_> = items.iter().map(OutlineItemKey::from_item).collect();
        self.collapsed_items
            .retain(|key| current_keys.contains(key));
        self.items = items;
        cx.notify();
    }

    fn toggle_item(&mut self, key: OutlineItemKey, cx: &mut Context<Self>) {
        if !self.collapsed_items.remove(&key) {
            self.collapsed_items.insert(key);
        }
        cx.notify();
    }

    fn visible_items(&self) -> Vec<(OutlineItem, bool, bool)> {
        visible_items(&self.items, &self.collapsed_items)
    }
}

/// 从大纲项与折叠集合推导可见行：折叠项隐藏其后所有更深层级的后代，但不影响同层兄弟。
fn visible_items(
    items: &[OutlineItem],
    collapsed_items: &HashSet<OutlineItemKey>,
) -> Vec<(OutlineItem, bool, bool)> {
    let mut visible = Vec::new();
    let mut collapsed_depth = None;
    for (index, item) in items.iter().enumerate() {
        if collapsed_depth.is_some_and(|depth| item.depth > depth) {
            continue;
        }
        collapsed_depth = None;
        let has_children = items
            .get(index + 1)
            .is_some_and(|next| next.depth > item.depth);
        let collapsed = has_children && collapsed_items.contains(&OutlineItemKey::from_item(item));
        visible.push((item.clone(), has_children, collapsed));
        if collapsed {
            collapsed_depth = Some(item.depth);
        }
    }
    visible
}

impl gpui::EventEmitter<PanelEvent> for OutlinePanel {}

impl Panel for OutlinePanel {
    fn icon() -> &'static str {
        "icons/list_tree.svg"
    }

    fn label() -> &'static str {
        "大纲"
    }

    fn persistent_name() -> &'static str {
        "outline"
    }

    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }

    fn set_active(&mut self, active: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.active = active;
        if active {
            self.invalidate_outline(cx);
        } else {
            // 不可见时取消挂起的重算，后台不再为不可见面板占用。
            self.refresh_task = None;
        }
    }
}

impl Render for OutlinePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = *color::current(cx);
        let search = SearchInput::new("outline", self.search_input.clone().into_any_element());
        let visible_items = self.visible_items();
        let items_len = visible_items.len();
        let active_editor = self.active_editor.clone();
        let weak_panel = cx.weak_entity();
        let weak_panel_for_toggle = weak_panel.clone();
        let list = uniform_list("outline-items", items_len, move |range, window, cx| {
            range
                .map(|index| {
                    let (item, has_children, collapsed) = visible_items[index].clone();
                    let editor = active_editor.clone();
                    let highlights = editor
                        .as_ref()
                        .map(|editor| editor.read(cx).outline_item_highlights(&item, cx))
                        .unwrap_or_default();
                    let panel = weak_panel_for_toggle.clone();
                    outline_item::render(
                        item,
                        outline_item::OutlineItemFold {
                            has_children,
                            collapsed,
                        },
                        highlights,
                        window,
                        cx,
                        move |key, cx| {
                            if let Some(panel) = panel.upgrade() {
                                panel.update(cx, |panel, cx| panel.toggle_item(key, cx));
                            }
                        },
                        move |item, window, cx| {
                            if let Some(editor) = editor.clone() {
                                editor.update(cx, |editor, cx| {
                                    if editor.navigate_to_outline_item(&item, cx) {
                                        window.focus(&editor.focus_handle(), cx);
                                    }
                                });
                            }
                        },
                    )
                })
                .collect()
        })
        .size_full()
        .track_scroll(&self.scroll_handle)
        .with_decoration(self.scrollbar.clone());

        let content = if items_len == 0 {
            let message = if self.active_editor.is_some() {
                "当前文件没有可用的大纲"
            } else {
                "没有打开的文件"
            };
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(colors.text_placeholder)
                .child(message)
                .into_any_element()
        } else {
            list.into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .key_context("outline")
            .text_color(colors.text)
            .child(div().w_full().p(space::S4).child(search))
            .child(content)
    }
}

#[cfg(test)]
#[path = "test/outline_tests.rs"]
mod tests;
