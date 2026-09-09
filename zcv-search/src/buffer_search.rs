//! 文件内搜索的会话控制器与界面。
//!
//! 搜索/替换输入框统一使用 `zcv-ui` 的 [`SearchInput`] / [`ReplaceInput`]（项目搜索等各视图各自装配，不共享本模块）；
//! 本模块只持有查询、选项、替换文本、可见性和输入焦点等会话状态，把用户操作组装成 [`SearchQuery`] 并派发给当前 Item 的 [`SearchableItemHandle`]；
//! 匹配与结果数据仍由具体 Item 持有。
//! 跨文件搜索执行位于 `project_search`。

use gpui::{
    App, Context, Entity, IntoElement, KeyContext, ParentElement, Render, Styled, Window, div,
    prelude::*,
};
use zcv_actions::{
    Backtab, ClearSearch, DeployBufferSearch, FindNext, FindPrevious, ReplaceAll, ReplaceNext,
    SelectAll, Tab, ToggleCaseSensitive, ToggleRegex, ToggleReplace, ToggleWholeWord,
};
use zcv_editor::{Editor, EditorEvent};
use zcv_text::SearchQuery;
use zcv_theme::{color, space};
use zcv_ui::{Button, MatchOption, MatchOptions, ReplaceInput, SearchInput};

use zcv_workspace::{
    Breadcrumbs, Direction, ItemHandle, PaneEvent, PreviewButton, SearchableItemHandle, Workspace,
};

pub(crate) struct DocumentToolbar {
    /// 键位上下文名（keymap 中的 `BufferSearchBar`）。
    context: &'static str,
    visible: bool,
    show_replace: bool,
    query: String,
    options: MatchOptions,
    replacement: String,
    /// 当前搜索目标（pane 的活动 item 的可搜索句柄）。
    active_item: Option<Box<dyn SearchableItemHandle>>,
    query_input: Option<Entity<Editor>>,
    replace_input: Option<Entity<Editor>>,
    input_subscriptions: Vec<gpui::Subscription>,
    active_item_subscription: Option<gpui::Subscription>,
    content_toolbar: gpui::AnyView,
    preview_button: Entity<PreviewButton>,
    breadcrumbs: Entity<Breadcrumbs>,
}

impl DocumentToolbar {
    pub(super) fn new(
        preview_button: Entity<PreviewButton>,
        breadcrumbs: Entity<Breadcrumbs>,
        cx: &mut Context<Self>,
    ) -> Self {
        // 输入框懒创建（首次打开搜索条时）：ErasedEditor 的创建与订阅都需要 window，且避免在无装配（如 Pane 单测）环境下构造。
        Self {
            context: "BufferSearchBar",
            visible: false,
            show_replace: false,
            query: String::new(),
            options: MatchOptions::default(),
            replacement: String::new(),
            active_item: None,
            query_input: None,
            replace_input: None,
            input_subscriptions: Vec::new(),
            active_item_subscription: None,
            content_toolbar: cx.entity().into(),
            preview_button,
            breadcrumbs,
        }
    }

    /// pane 的活动 item 变化时同步搜索目标；搜索条可见时在新 item 上重跑当前 query。
    pub(super) fn set_active_item(
        &mut self,
        item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(editor) = item.and_then(|item| item.act_as::<Editor>(cx)) {
            let content_toolbar = self.content_toolbar.clone();
            editor.update(cx, |editor, cx| {
                editor.set_content_toolbar_view(content_toolbar, cx)
            });
        }
        self.preview_button.update(cx, |preview_button, cx| {
            preview_button.set_active_item(item, window, cx)
        });
        self.breadcrumbs
            .update(cx, |breadcrumbs, cx| breadcrumbs.set_item(item, cx));
        let new_item = item.and_then(|item| item.as_searchable(cx));
        // 重建订阅：item 切换后旧订阅失效（emit 方已释放）。
        self.active_item_subscription = None;
        if let Some(item) = &new_item {
            let weak = cx.weak_entity();
            self.active_item_subscription = Some(item.subscribe_to_search_events(
                window,
                cx,
                Box::new(move |_, _window, cx| {
                    if let Some(search_bar) = weak.upgrade() {
                        // 计数渲染时从 item 读取，这里只需触发重绘。
                        search_bar.update(cx, |_, cx| cx.notify());
                    }
                }),
            ));
        }
        let item_changed = new_item.as_ref().is_none_or(|new| {
            self.active_item
                .as_ref()
                .is_none_or(|old| old.item_id() != new.item_id())
        });
        self.active_item = new_item;
        if !self
            .active_item
            .as_ref()
            .is_some_and(|item| item.supports_replace(cx))
        {
            self.show_replace = false;
        }
        if item_changed && self.visible {
            self.run_search(window, cx);
        }
    }

    /// 懒创建输入框并建立事件订阅（首次打开搜索条时调用；输入变化自动重搜）。
    fn ensure_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.query_input.is_some() {
            return;
        }
        // 输入框懒创建（首次打开搜索条时）：Editor 的创建与订阅都需要 window，且避免在无装配（如 Pane 单测）环境下构造。
        let query_input = cx.new(|cx| Editor::auto_height(1, Some(4), cx));
        let replace_input = cx.new(|cx| Editor::auto_height(1, Some(4), cx));
        query_input.update(cx, |editor, cx| editor.set_placeholder_text("搜索...", cx));
        replace_input.update(cx, |editor, cx| {
            editor.set_placeholder_text("替换为...", cx)
        });
        let weak = cx.weak_entity();
        self.input_subscriptions
            .push(window.subscribe(&query_input, cx, {
                let weak = weak.clone();
                move |_, event: &EditorEvent, window, cx| {
                    if *event != EditorEvent::Edited {
                        return;
                    }
                    if let Some(search_bar) = weak.upgrade() {
                        search_bar.update(cx, |search_bar, cx| {
                            search_bar.run_search(window, cx);
                        });
                    }
                }
            }));
        self.input_subscriptions
            .push(window.subscribe(&replace_input, cx, {
                let weak = weak.clone();
                move |_, event: &EditorEvent, _window, cx| {
                    if *event != EditorEvent::Edited {
                        return;
                    }
                    if let Some(search_bar) = weak.upgrade() {
                        search_bar.update(cx, |search_bar, cx| {
                            search_bar.replacement = search_bar
                                .replace_input
                                .as_ref()
                                .map_or(String::new(), |input| input.read(cx).text(cx));
                        });
                    }
                }
            }));
        self.query_input = Some(query_input);
        self.replace_input = Some(replace_input);
    }

    /// 部署搜索条：无论当前状态，一律打开并把焦点移到搜索框；
    /// `query_seed` 为调用方预先提取的建议（项目搜索必须在切换活动 Item 前提取）；
    /// 无种子时向活动 Item 请求查询建议（选区文本）；
    /// cmd-f 只开不关；关闭由工具栏「搜索」按钮（开/关）或 esc 触发。
    pub(super) fn deploy(
        &mut self,
        query_seed: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_visible = self.visible;
        self.visible = true;
        self.ensure_inputs(window, cx);
        let query_input = self.query_input.as_ref().unwrap();
        let seed = query_seed.or_else(|| {
            self.active_item
                .as_ref()
                .and_then(|item| item.query_suggestion(cx))
        });
        let seeded = seed.is_some();
        if let Some(seed) = seed {
            // 正则模式下先转义原始文本，避免选区中的元字符改变查询语义。
            self.query = if self.options.regex {
                regex::escape(&seed)
            } else {
                seed
            };
        }
        query_input.update(cx, |editor, cx| editor.set_text(&self.query, cx));
        window.focus(&query_input.read(cx).focus_handle(), cx);
        // 全选查询文本：直接击键即可整体替换。
        window.dispatch_action(Box::new(SelectAll), cx);
        if !was_visible || seeded {
            self.run_search(window, cx);
        }
        cx.notify();
    }

    /// 关闭搜索条（esc / ✕）：清空搜索状态并把焦点还给活动 item。
    pub(super) fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = false;
        if let Some(item) = &self.active_item {
            item.clear_search(window, cx);
            window.focus(&item.item_focus_handle(cx), cx);
        }
        cx.notify();
    }

    /// 工具栏「搜索」按钮的开/关切换:搜索条打开时关闭,未打开时部署。
    fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible {
            self.close(window, cx);
        } else {
            self.deploy(None, window, cx);
        }
    }

    /// 用当前 query 与选项在活动 item 上执行搜索；计数由渲染时读取 item 状态。
    fn run_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.query = self
            .query_input
            .as_ref()
            .map_or(String::new(), |input| input.read(cx).text(cx));
        let Some(item) = &self.active_item else {
            return;
        };
        let query = SearchQuery {
            query: self.query.clone(),
            case_sensitive: self.options.case_sensitive,
            whole_word: self.options.whole_word,
            regex: self.options.regex,
        };
        item.search(&query, window, cx);
        cx.notify();
    }

    fn move_active(&mut self, direction: Direction, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = &self.active_item {
            item.activate_match_in_direction(direction, 1, window, cx);
        }
        cx.notify();
    }

    fn toggle_option(&mut self, option: MatchOption, window: &mut Window, cx: &mut Context<Self>) {
        self.options = self.options.toggled(option);
        self.run_search(window, cx);
    }

    fn toggle_replace_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self
            .active_item
            .as_ref()
            .is_some_and(|item| item.supports_replace(cx))
        {
            return;
        }
        let closing = self.show_replace;
        self.show_replace = !self.show_replace;
        if self.show_replace
            && let Some(replace_input) = &self.replace_input
        {
            replace_input.update(cx, |editor, cx| editor.set_text(&self.replacement, cx));
            // 打开替换行时默认聚焦替换输入框。
            window.focus(&replace_input.read(cx).focus_handle(), cx);
        }
        // 收起替换行时把焦点还给搜索输入框。
        if closing && let Some(query_input) = &self.query_input {
            window.focus(&query_input.read(cx).focus_handle(), cx);
        }
        cx.notify();
    }

    fn replace_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replacement = self
            .replace_input
            .as_ref()
            .map_or(String::new(), |input| input.read(cx).text(cx));
        if let Some(item) = &self.active_item
            && item.replace_current(&self.replacement, window, cx)
        {
            // 替换触发编辑 → Item 侧重搜并 emit；这里跟随活动匹配前移一位。
            self.move_active(Direction::Next, window, cx);
        }
    }

    fn replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replacement = self
            .replace_input
            .as_ref()
            .map_or(String::new(), |input| input.read(cx).text(cx));
        if let Some(item) = &self.active_item {
            item.replace_all(&self.replacement, window, cx);
        }
    }

    /// 焦点在 query 输入框 → 替换输入框 → 活动 item 间循环。
    fn cycle_focus(&mut self, direction: Direction, window: &mut Window, cx: &mut Context<Self>) {
        let mut handles = vec![self.query_input.as_ref().unwrap().read(cx).focus_handle()];
        if self.show_replace {
            handles.push(self.replace_input.as_ref().unwrap().read(cx).focus_handle());
        }
        if let Some(item) = &self.active_item {
            handles.push(item.item_focus_handle(cx));
        }
        let current = match handles.iter().position(|focus| focus.is_focused(window)) {
            Some(index) => index,
            None => return,
        };
        let next = match direction {
            Direction::Next => (current + 1) % handles.len(),
            Direction::Prev => (current + handles.len() - 1) % handles.len(),
        };
        window.focus(&handles[next], cx);
        cx.stop_propagation();
    }
}

impl Render for DocumentToolbar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 面包屑行:始终作为第一行;搜索条打开时搜索行追加在它下方,不替换原内容。
        let colors = color::current(cx);
        let weak = cx.weak_entity();
        let breadcrumbs_line = div()
            .w_full()
            .flex()
            .items_center()
            .gap(space::S6)
            .on_action(cx.listener(Self::handle_deploy))
            .child(div().flex_1().min_w_0().child(self.breadcrumbs.clone()))
            .child(self.preview_button.clone())
            // 搜索按钮:未打开时提示 cmd-f(搜索);
            // 打开后按钮语义为「关闭」,高亮并提示 esc(关闭搜索)。
            .child({
                let search_toggle =
                    Button::icon("toolbar-file-search", "icons/magnifying_glass.svg")
                        .label(if self.visible {
                            "关闭搜索"
                        } else {
                            "搜索"
                        })
                        .color(if self.visible {
                            colors.icon_accent
                        } else {
                            colors.text_muted
                        })
                        .on_click(move |_, window, cx| {
                            if let Some(toolbar) = weak.upgrade() {
                                toolbar.update(cx, |toolbar, cx| toolbar.toggle_search(window, cx));
                            }
                        });
                if self.visible {
                    search_toggle.shortcut(&ClearSearch, cx)
                } else {
                    search_toggle.shortcut(&DeployBufferSearch, cx)
                }
            })
            .into_any_element();
        if !self.visible {
            return breadcrumbs_line;
        }
        // 按钮点击经弱引用更新组件状态。
        let weak = cx.weak_entity();
        let toggle_replace = {
            let weak = weak.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                if let Some(search_bar) = weak.upgrade() {
                    search_bar.update(cx, |search_bar, cx| {
                        search_bar.toggle_replace_mode(window, cx);
                    });
                }
            }
        };
        let on_previous = {
            let weak = weak.clone();
            move |window: &mut Window, cx: &mut App| {
                if let Some(search_bar) = weak.upgrade() {
                    search_bar.update(cx, |search_bar, cx| {
                        search_bar.move_active(Direction::Prev, window, cx);
                    });
                }
            }
        };
        let on_next = {
            let weak = weak.clone();
            move |window: &mut Window, cx: &mut App| {
                if let Some(search_bar) = weak.upgrade() {
                    search_bar.update(cx, |search_bar, cx| {
                        search_bar.move_active(Direction::Next, window, cx);
                    });
                }
            }
        };
        let on_toggle = {
            let weak = weak.clone();
            move |option: MatchOption, window: &mut Window, cx: &mut App| {
                if let Some(search_bar) = weak.upgrade() {
                    search_bar.update(cx, |search_bar, cx| {
                        search_bar.toggle_option(option, window, cx);
                    });
                }
            }
        };
        let replace_action = {
            let weak = weak.clone();
            move |window: &mut Window, cx: &mut App| {
                if let Some(search_bar) = weak.upgrade() {
                    search_bar.update(cx, |search_bar, cx| {
                        search_bar.replace_next(window, cx);
                    });
                }
            }
        };
        let replace_all_action = {
            let weak = weak.clone();
            move |window: &mut Window, cx: &mut App| {
                if let Some(search_bar) = weak.upgrade() {
                    search_bar.update(cx, |search_bar, cx| {
                        search_bar.replace_all(window, cx);
                    });
                }
            }
        };
        // 计数从 item 实时读取（单一数据源），渲染不持有计数副本。
        let (match_count, active_match_index) = self
            .active_item
            .as_ref()
            .map_or((0, None), |item| item.search_count(cx));
        let supports_replace = self
            .active_item
            .as_ref()
            .is_some_and(|item| item.supports_replace(cx));
        // 搜索行:通用搜索输入框(内部插槽:匹配选项;外部插槽:计数与上/下跳转)。
        let search_line = SearchInput::new(
            "buffer-search",
            self.query_input
                .as_ref()
                .unwrap()
                .clone()
                .into_any_element(),
        )
        .options(self.options)
        .on_toggle(on_toggle)
        .count(active_match_index, match_count)
        .on_previous(on_previous)
        .on_next(on_next)
        // 追加插槽:替换开关(是否插入由调用方决定;不支持替换的目标上显示禁用态)。
        // 关闭入口:工具栏「搜索」按钮(开/关)与 esc,行内不再放关闭按钮。
        .external(
            Button::icon("search-toggle-replace", "icons/replace.svg")
                .label("替换")
                .shortcut(&ToggleReplace, cx)
                .color(if self.show_replace {
                    colors.icon_accent
                } else {
                    colors.text_muted
                })
                .on_click(toggle_replace)
                .disabled(!supports_replace),
        )
        .into_any_element();
        // 替换行:通用替换输入框(外部插槽:替换当前匹配 / 全部替换)。
        let replacement_line = (self.show_replace && supports_replace).then(|| {
            ReplaceInput::new(
                "buffer-replace",
                self.replace_input
                    .as_ref()
                    .unwrap()
                    .clone()
                    .into_any_element(),
            )
            .on_replace(replace_action)
            .on_replace_all(replace_all_action)
            .into_any_element()
        });
        // 替换类快捷键仅在替换输入框聚焦时生效:按焦点动态附加 in_replace 标签, keymap 据此区分查询框与替换框。
        let mut key_context = KeyContext::new_with_defaults();
        key_context.add(self.context);
        if self
            .replace_input
            .as_ref()
            .is_some_and(|input| input.read(cx).focus_handle().is_focused(window))
        {
            key_context.add("in_replace");
        }
        div()
            .key_context(key_context)
            .flex()
            .flex_col()
            .gap(space::S6)
            .on_action(cx.listener(Self::handle_find_next))
            .on_action(cx.listener(Self::handle_find_previous))
            .on_action(cx.listener(Self::handle_toggle_replace))
            .on_action(cx.listener(Self::handle_replace_next))
            .on_action(cx.listener(Self::handle_replace_all))
            .on_action(cx.listener(Self::handle_clear_search))
            .on_action(cx.listener(Self::handle_toggle_case_sensitive))
            .on_action(cx.listener(Self::handle_toggle_whole_word))
            .on_action(cx.listener(Self::handle_toggle_regex))
            .on_action(cx.listener(Self::handle_tab))
            .on_action(cx.listener(Self::handle_backtab))
            // 第一行:面包屑行。
            .child(breadcrumbs_line)
            // 搜索行:通用搜索输入框(内部插槽:匹配选项;外部插槽:计数、上/下一个匹配,追加替换开关与关闭)。
            .child(search_line)
            // 替换行:替换开关展开后显示,通用替换输入框(外部插槽:替换 / 全部替换)。
            .when_some(replacement_line, |this, line| this.child(line))
            .into_any_element()
    }
}

// ═══ DocumentToolbar actions（keymap BufferSearchBar 上下文绑定）═══

impl DocumentToolbar {
    fn handle_deploy(
        &mut self,
        _: &DeployBufferSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.deploy(None, window, cx);
    }

    fn handle_find_next(&mut self, _: &FindNext, window: &mut Window, cx: &mut Context<Self>) {
        self.move_active(Direction::Next, window, cx);
    }

    fn handle_find_previous(
        &mut self,
        _: &FindPrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_active(Direction::Prev, window, cx);
    }

    fn handle_toggle_replace(
        &mut self,
        _: &ToggleReplace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_replace_mode(window, cx);
    }

    fn handle_replace_next(
        &mut self,
        _: &ReplaceNext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_next(window, cx);
    }

    fn handle_replace_all(&mut self, _: &ReplaceAll, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_all(window, cx);
    }

    fn handle_clear_search(
        &mut self,
        _: &ClearSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close(window, cx);
    }

    fn handle_toggle_case_sensitive(
        &mut self,
        _: &ToggleCaseSensitive,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_option(MatchOption::CaseSensitive, window, cx);
    }

    fn handle_toggle_whole_word(
        &mut self,
        _: &ToggleWholeWord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_option(MatchOption::WholeWord, window, cx);
    }

    fn handle_toggle_regex(
        &mut self,
        _: &ToggleRegex,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_option(MatchOption::Regex, window, cx);
    }

    fn handle_tab(&mut self, _: &Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_focus(Direction::Next, window, cx);
    }

    fn handle_backtab(&mut self, _: &Backtab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_focus(Direction::Prev, window, cx);
    }
}

pub(super) fn install(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Entity<DocumentToolbar> {
    let pane = workspace.pane().clone();
    let preview_button = cx.new(|_| PreviewButton::new(pane.downgrade()));
    let breadcrumbs = cx.new(|_| Breadcrumbs::new(workspace.project().clone()));
    let document_toolbar = cx.new(|cx| DocumentToolbar::new(preview_button, breadcrumbs, cx));
    cx.subscribe_in(&pane, window, {
        let document_toolbar = document_toolbar.clone();
        move |_, pane, event, window, cx| {
            if matches!(
                event,
                PaneEvent::AddItem { .. }
                    | PaneEvent::ActivateItem { .. }
                    | PaneEvent::RemovedItem { .. }
            ) {
                let active_item = pane.read(cx).active_item().map(ItemHandle::boxed_clone);
                document_toolbar.update(cx, |toolbar, cx| {
                    toolbar.set_active_item(active_item.as_deref(), window, cx)
                });
            }
        }
    })
    .detach();
    document_toolbar
}
