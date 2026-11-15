//! 文件搜索按钮与按需展开的搜索行共享同一份 SearchBar 会话。

use std::sync::Arc;

use gpui::{
    App, Context, Entity, EventEmitter, ParentElement, Render, Subscription, Window, div,
    prelude::*,
};
use zcv_actions::{ClearSearch, DeployFind, DeployReplace};
use zcv_editor::Editor;
use zcv_keymap::display_shortcut;
use zcv_language::LanguageRegistry;
use zcv_theme::color;
use zcv_ui::Button;
use zcv_workspace::{
    ItemHandle, ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView, Workspace,
};

use crate::{SearchBar, SearchBarConfig, SearchBarPresentation, SearchBarSlots};

pub(crate) struct DocumentSearchButton {
    search_bar: Entity<SearchBar>,
}

impl DocumentSearchButton {
    pub(super) fn new(language_registry: Arc<LanguageRegistry>, cx: &mut Context<Self>) -> Self {
        let search_bar = cx.new(move |cx| {
            SearchBar::new(
                SearchBarConfig {
                    id_prefix: "buffer-search",
                    key_context: "BufferSearchBar",
                    query_placeholder: "搜索...",
                    replace_placeholder: "替换为...",
                    presentation: SearchBarPresentation::OnDemand,
                },
                language_registry,
                cx,
            )
        });
        Self { search_bar }
    }

    pub(super) fn search_row(&self, cx: &mut App) -> Entity<DocumentSearchRow> {
        let search_bar = self.search_bar.clone();
        cx.new(|cx| DocumentSearchRow::new(search_bar, cx))
    }

    /// 活动普通文档可搜索时部署搜索栏，并把焦点移到搜索框。
    pub(super) fn deploy(
        &mut self,
        query_seed: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_bar.update(cx, |bar, cx| {
            if bar.has_target() {
                bar.deploy(query_seed, window, cx);
            }
        });
    }

    /// 在活动普通文档上切换替换行；搜索栏未打开时以替换方式部署。
    pub(super) fn toggle_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_bar
            .update(cx, |bar, cx| bar.toggle_replace(window, cx));
    }
}

impl EventEmitter<ToolbarItemEvent> for DocumentSearchButton {}

impl ToolbarItemView for DocumentSearchButton {
    fn set_active_pane_item(
        &mut self,
        item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        // 只有直接打开的 Editor Item 使用这份 Pane 搜索会话。
        // 组合 Item 即使暴露内层 Editor，也不借用这份 Pane 搜索会话。
        let editor_item = item.filter(|item| {
            item.act_as::<Editor>(cx)
                .is_some_and(|editor| editor.entity_id() == item.item_id())
        });
        let target = editor_item
            .and_then(|item| item.as_searchable(cx))
            .map(|handle| handle.downgrade());
        self.search_bar
            .update(cx, |bar, cx| bar.set_target(target, window, cx));
        if self.search_bar.read(cx).has_target() {
            ToolbarItemLocation::PrimaryRight
        } else {
            ToolbarItemLocation::Hidden
        }
    }
}

impl Render for DocumentSearchButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = color::current(cx);
        let search_bar = self.search_bar.clone();
        let visible = search_bar.read(cx).visible();
        let weak = search_bar.downgrade();
        let search_toggle = Button::icon("toolbar-file-search", "icons/magnifying_glass.svg")
            .label(if visible { "关闭搜索" } else { "搜索" })
            .color(if visible {
                colors.icon_accent
            } else {
                colors.text_muted
            })
            .on_click(move |_, window, cx| {
                if let Some(bar) = weak.upgrade() {
                    bar.update(cx, |bar, cx| bar.toggle_search(window, cx));
                }
            });
        div()
            .debug_selector(|| "toolbar-file-search".into())
            .on_action(cx.listener(Self::handle_deploy))
            .on_action(cx.listener(Self::handle_toggle_replace))
            .child(if visible {
                search_toggle.shortcut(display_shortcut(&ClearSearch, cx))
            } else {
                search_toggle.shortcut(display_shortcut(&DeployFind, cx))
            })
    }
}

pub(super) struct DocumentSearchRow {
    search_bar: Entity<SearchBar>,
    _subscription: Subscription,
}

impl DocumentSearchRow {
    fn new(search_bar: Entity<SearchBar>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&search_bar, |_, bar, cx| {
            cx.emit(ToolbarItemEvent::ChangeLocation(Self::location(
                bar.read(cx),
            )));
            cx.notify();
        });
        Self {
            search_bar,
            _subscription: subscription,
        }
    }

    fn location(bar: &SearchBar) -> ToolbarItemLocation {
        if bar.visible() && bar.has_target() {
            ToolbarItemLocation::Secondary
        } else {
            ToolbarItemLocation::Hidden
        }
    }
}

impl EventEmitter<ToolbarItemEvent> for DocumentSearchRow {}

impl ToolbarItemView for DocumentSearchRow {
    fn set_active_pane_item(
        &mut self,
        _: Option<&dyn ItemHandle>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        Self::location(self.search_bar.read(cx))
    }
}

impl Render for DocumentSearchRow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.search_bar.update(cx, |bar, cx| {
            bar.render(SearchBarSlots::default(), window, cx)
        })
    }
}

impl DocumentSearchButton {
    fn handle_deploy(&mut self, _: &DeployFind, window: &mut Window, cx: &mut Context<Self>) {
        self.deploy(None, window, cx);
    }

    fn handle_toggle_replace(
        &mut self,
        _: &DeployReplace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_replace(window, cx);
    }
}

pub(super) fn install(
    workspace: &mut Workspace,
    cx: &mut Context<Workspace>,
) -> Entity<DocumentSearchButton> {
    let language_registry = workspace.project().read(cx).language_registry();
    cx.new(|cx| DocumentSearchButton::new(language_registry, cx))
}
