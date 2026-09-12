//! 语法大纲面板。
//!
//! 面板持有当前活动编辑器、筛选结果和折叠状态；
//! 语法数据由 `zcv-editor` 提供，行的通用树几何由 `zcv-ui` 提供。

use std::collections::HashSet;

use gpui::{
    App, Context, Entity, FocusHandle, Render, UniformListScrollHandle, Window, div, prelude::*,
    uniform_list,
};
use zcv_editor::{Editor, EditorEvent};
use zcv_language::OutlineItem;
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
    items: Vec<OutlineItem>,
    collapsed_items: HashSet<OutlineItemKey>,
    scroll_handle: UniformListScrollHandle,
    scrollbar: Scrollbar<UniformListScrollHandle>,
}

impl OutlinePanel {
    pub fn new(pane: Entity<Pane>, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(Editor::single_line);
        search_input.update(cx, |editor, cx| {
            editor.set_placeholder_text("筛选大纲…", cx);
        });
        let search_subscription =
            cx.subscribe(&search_input, |panel, _input, event: &EditorEvent, cx| {
                if *event == EditorEvent::Edited {
                    panel.refresh_items(cx);
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
                cx.observe(editor, |panel, _, cx| {
                    panel.refresh_items(cx);
                })
            });
            self.active_editor = next_editor;
        }
        self.refresh_items(cx);
    }

    fn refresh_items(&mut self, cx: &mut Context<Self>) {
        let items = self
            .active_editor
            .as_ref()
            .map(|editor| {
                let query = self.search_input.read(cx).text(cx);
                editor.read(cx).outline_items_matching(&query)
            })
            .unwrap_or_default();
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
        let mut visible = Vec::new();
        let mut collapsed_depth = None;
        for (index, item) in self.items.iter().enumerate() {
            if collapsed_depth.is_some_and(|depth| item.depth > depth) {
                continue;
            }
            collapsed_depth = None;
            let has_children = self
                .items
                .get(index + 1)
                .is_some_and(|next| next.depth > item.depth);
            let collapsed = has_children
                && self
                    .collapsed_items
                    .contains(&OutlineItemKey::from_item(item));
            visible.push((item.clone(), has_children, collapsed));
            if collapsed {
                collapsed_depth = Some(item.depth);
            }
        }
        visible
    }
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
        let list = uniform_list("outline-items", items_len, move |range, _window, cx| {
            range
                .map(|index| {
                    let (item, has_children, collapsed) = visible_items[index].clone();
                    let editor = active_editor.clone();
                    let highlights = editor
                        .as_ref()
                        .map(|editor| editor.read(cx).outline_item_highlights(&item))
                        .unwrap_or_default();
                    let panel = weak_panel_for_toggle.clone();
                    outline_item::render(
                        item,
                        has_children,
                        collapsed,
                        highlights,
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
