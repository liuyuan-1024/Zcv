//! 文件内搜索工具项：面包屑、预览入口与共享搜索栏。
//!
//! 搜索会话（查询/替换输入、匹配选项、可见性、替换开关、按键接线与命中导航）
//! 由 zcv-search 的共享 SearchBar 承担；
//! 本模块只负责工具项位置、活动 Item 的目标解析，以及面包屑与预览入口。
//! 面包屑与搜索栏是两个独立的工具项元素，搜索栏独立渲染。

use gpui::{Context, Entity, EventEmitter, ParentElement, Render, Styled, Window, div, prelude::*};
use zcv_actions::{ClearSearch, DeployBufferSearch};
use zcv_editor::Editor;
use zcv_theme::{color, space};
use zcv_ui::Button;
use zcv_workspace::{
    Breadcrumbs, ItemHandle, PreviewButton, ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView,
    Workspace,
};

use crate::{SearchBar, SearchBarConfig, SearchBarSlots};

pub(crate) struct DocumentToolbar {
    search_bar: Entity<SearchBar>,
    preview_button: Entity<PreviewButton>,
    breadcrumbs: Entity<Breadcrumbs>,
}

impl DocumentToolbar {
    pub(super) fn new(
        preview_button: Entity<PreviewButton>,
        breadcrumbs: Entity<Breadcrumbs>,
        cx: &mut Context<Self>,
    ) -> Self {
        let search_bar = cx.new(|cx| {
            SearchBar::new(
                SearchBarConfig {
                    id_prefix: "buffer-search",
                    key_context: "BufferSearchBar",
                    supports_replace: true,
                    query_placeholder: "搜索...",
                    replace_placeholder: "替换为...",
                    dismissible: true,
                },
                cx,
            )
        });
        Self {
            search_bar,
            preview_button,
            breadcrumbs,
        }
    }

    /// 部署搜索条：无论当前状态一律打开并把焦点移到搜索框。
    pub(super) fn deploy(
        &mut self,
        query_seed: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_bar
            .update(cx, |bar, cx| bar.deploy(query_seed, window, cx));
    }
}

impl EventEmitter<ToolbarItemEvent> for DocumentToolbar {}

impl ToolbarItemView for DocumentToolbar {
    /// 活动 Item 变化时同步搜索目标与工具栏内容，并返回本工具项的显示位置。
    ///
    /// 只有使用编辑器通用文档工具栏的编辑器 Item 显示本工具区；
    /// 预览、差异、提交图等 Item 由各自的工具项承担工具区。
    fn set_active_pane_item(
        &mut self,
        item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        self.preview_button.update(cx, |preview_button, cx| {
            preview_button.set_active_item(item, window, cx)
        });
        self.breadcrumbs
            .update(cx, |breadcrumbs, cx| breadcrumbs.set_item(item, cx));
        // 搜索栏保存弱目标：目标释放后搜索自然停止，不延长目标生命周期。
        let target = item
            .and_then(|item| item.as_searchable(cx))
            .map(|handle| handle.downgrade());
        self.search_bar
            .update(cx, |bar, cx| bar.set_target(target, window, cx));
        // 编辑器通用文档工具栏只服务本身就是编辑器的 Item。
        // 预览、项目搜索、差异等 Item 只通过 `act_as_type` 暴露内层编辑器，自身不是编辑器实体；
        // 它们由各自的工具项承担工具区，避免通用文档工具栏与专用工具项重复显示。
        let is_editor_item = item.is_some_and(|item| {
            item.act_as::<Editor>(cx)
                .is_some_and(|editor| editor.entity_id() == item.item_id())
        });
        if is_editor_item {
            ToolbarItemLocation::Secondary
        } else {
            ToolbarItemLocation::Hidden
        }
    }
}

impl Render for DocumentToolbar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 面包屑行：始终作为第一行；搜索条打开时搜索行追加在它下方，不替换原内容。
        let colors = color::current(cx);
        let search_bar = self.search_bar.clone();
        let visible = search_bar.read(cx).visible();
        let weak = search_bar.downgrade();
        let breadcrumbs_line = div()
            .w_full()
            .flex()
            .items_center()
            .gap(space::S6)
            .on_action(cx.listener(Self::handle_deploy))
            .child(div().flex_1().min_w_0().child(self.breadcrumbs.clone()))
            .child(self.preview_button.clone())
            // 搜索按钮：未打开时提示 cmd-f（搜索）；
            // 打开后按钮语义为「关闭」，高亮并提示 esc（关闭搜索）。
            .child({
                let search_toggle =
                    Button::icon("toolbar-file-search", "icons/magnifying_glass.svg")
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
                if visible {
                    search_toggle.shortcut(zcv_keymap::display_shortcut(&ClearSearch, cx))
                } else {
                    search_toggle.shortcut(zcv_keymap::display_shortcut(&DeployBufferSearch, cx))
                }
            })
            .into_any_element();
        let container = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(space::S6)
            .child(breadcrumbs_line);
        if !visible {
            return container.into_any_element();
        }
        let search = search_bar.update(cx, |bar, cx| {
            bar.render(SearchBarSlots::default(), window, cx)
        });
        container.child(search).into_any_element()
    }
}

impl DocumentToolbar {
    fn handle_deploy(
        &mut self,
        _: &DeployBufferSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.deploy(None, window, cx);
    }
}

pub(super) fn install(
    workspace: &mut Workspace,
    cx: &mut Context<Workspace>,
) -> Entity<DocumentToolbar> {
    let pane = workspace.pane().clone();
    let preview_button = cx.new(|_| PreviewButton::new(pane.downgrade()));
    let breadcrumbs = cx.new(|_| Breadcrumbs::new(workspace.project().clone()));
    cx.new(|cx| DocumentToolbar::new(preview_button, breadcrumbs, cx))
}
