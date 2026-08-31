//! 文件内搜索与项目搜索共享的搜索会话控制器及界面。
//!
//! 持有查询、选项、替换文本、可见性和输入焦点等会话状态，把用户操作组装成 [`SearchQuery`] 并派发给当前 Item 的 [`SearchableItemHandle`]；
//! 匹配与结果数据仍由具体 Item 持有。
//! Pane/Workspace 接线位于 `buffer_search`，跨文件搜索执行位于 `project_search`。

use gpui::{
    AnyElement, App, Component, Context, Entity, IntoElement, KeyContext, ParentElement, Render,
    RenderOnce, SharedString, Styled, Window, div, prelude::*,
};
use zcv_actions::{
    Backtab, ClearSearch, FindNext, FindPrevious, ReplaceAll, ReplaceNext, SelectAll, Tab,
    ToggleCaseSensitive, ToggleRegex, ToggleReplace, ToggleWholeWord,
};
use zcv_editor::{Editor, EditorEvent};
use zcv_text::SearchQuery;
use zcv_theme::{color, space, typography};
use zcv_ui::Button;

use zcv_workspace::{
    Direction, ItemHandle, SearchableItemHandle, ToolbarItemEvent, ToolbarItemLocation,
};

#[derive(Clone, Copy)]
enum SearchOption {
    CaseSensitive,
    WholeWord,
    Regex,
}

/// 查询选项按钮的激活状态（仅查询框右侧渲染）。
struct QueryOptions {
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
}

/// SearchBar 输入框共享外观：带边框的多行容器 + 可选选项按钮。
struct SearchInput {
    id: SharedString,
    input: AnyElement,
    options: Option<QueryOptions>,
}

impl SearchInput {
    fn new(id: impl Into<SharedString>, input: AnyElement, options: Option<QueryOptions>) -> Self {
        Self {
            id: id.into(),
            input,
            options,
        }
    }
}

impl IntoElement for SearchInput {
    type Element = Component<Self>;

    fn into_element(self) -> Self::Element {
        Component::new(self)
    }
}

impl RenderOnce for SearchInput {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = color::current(cx);
        let case_id = (self.id.clone(), 0);
        let word_id = (self.id.clone(), 1);
        let regex_id = (self.id, 2);

        div()
            .flex_1()
            .flex()
            .items_center()
            .min_h_8()
            .px(space::S6)
            .rounded_sm()
            .border_1()
            .border_color(colors.border)
            .child(self.input)
            .when_some(self.options, |this, options| {
                this.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(space::S4)
                        .child(
                            Button::icon(case_id, "icons/case_sensitive.svg")
                                .label("区分大小写")
                                .shortcut(&ToggleCaseSensitive, cx)
                                .color(if options.case_sensitive {
                                    colors.icon_accent
                                } else {
                                    colors.text_muted
                                })
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(ToggleCaseSensitive), cx)
                                }),
                        )
                        .child(
                            Button::icon(word_id, "icons/whole_word.svg")
                                .label("整词匹配")
                                .shortcut(&ToggleWholeWord, cx)
                                .color(if options.whole_word {
                                    colors.icon_accent
                                } else {
                                    colors.text_muted
                                })
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(ToggleWholeWord), cx)
                                }),
                        )
                        .child(
                            Button::icon(regex_id, "icons/regex.svg")
                                .label("正则表达式")
                                .shortcut(&ToggleRegex, cx)
                                .color(if options.regex {
                                    colors.icon_accent
                                } else {
                                    colors.text_muted
                                })
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(ToggleRegex), cx)
                                }),
                        ),
                )
            })
    }
}

pub(crate) struct SearchBar {
    /// 键位上下文名，由消费方注入（文件内搜索为 BufferSearchBar，项目搜索为 ProjectSearchBar）。
    context: &'static str,
    visible: bool,
    show_replace: bool,
    query: String,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    replacement: String,
    /// 当前搜索目标（pane 的活动 item 的可搜索句柄）。
    active_item: Option<Box<dyn SearchableItemHandle>>,
    query_input: Option<Entity<Editor>>,
    replace_input: Option<Entity<Editor>>,
    input_subscriptions: Vec<gpui::Subscription>,
    active_item_subscription: Option<gpui::Subscription>,
}

impl gpui::EventEmitter<ToolbarItemEvent> for SearchBar {}

impl SearchBar {
    pub(crate) fn new(context: &'static str, _cx: &mut Context<Self>) -> Self {
        // 输入框懒创建（首次打开搜索条时）：ErasedEditor 的创建与订阅都需要 window，且避免在无装配（如 Pane 单测）环境下构造。
        Self {
            context,
            visible: false,
            show_replace: false,
            query: String::new(),
            case_sensitive: false,
            whole_word: false,
            regex: false,
            replacement: String::new(),
            active_item: None,
            query_input: None,
            replace_input: None,
            input_subscriptions: Vec::new(),
            active_item_subscription: None,
        }
    }

    /// pane 的活动 item 变化时同步搜索目标；搜索条可见时在新 item 上重跑当前 query。
    pub(super) fn set_active_item(
        &mut self,
        item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        if self
            .active_item
            .as_ref()
            .is_some_and(|item| !item.supports_replace(cx))
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

    /// 部署搜索条（cmd-f / 工具栏按钮）：无论当前状态，一律打开并把焦点移到搜索框；
    /// `query_seed` 为调用方预先提取的建议（项目搜索必须在切换活动 Item 前提取）；
    /// 无种子时向活动 Item 请求查询建议（选区文本）；
    /// 关闭只由 esc / ✕ 触发（cmd-f 永不关闭搜索条）。
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
            self.query = if self.regex {
                regex::escape(&seed)
            } else {
                seed
            };
        }
        query_input.update(cx, |editor, cx| editor.set_text(&self.query, cx));
        window.focus(&query_input.read(cx).focus_handle());
        // 全选查询文本：直接击键即可整体替换。
        window.dispatch_action(Box::new(SelectAll), cx);
        if !was_visible || seeded {
            self.run_search(window, cx);
        }
        cx.emit(ToolbarItemEvent::ChangeLocation(
            ToolbarItemLocation::Secondary,
        ));
        cx.notify();
    }

    pub(super) fn location(&self) -> ToolbarItemLocation {
        if self.visible {
            ToolbarItemLocation::Secondary
        } else {
            ToolbarItemLocation::Hidden
        }
    }

    /// 关闭搜索条（esc / ✕）：清空搜索状态并把焦点还给活动 item。
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = false;
        if let Some(item) = &self.active_item {
            item.clear_search(window, cx);
            window.focus(&item.item_focus_handle(cx));
        }
        cx.emit(ToolbarItemEvent::ChangeLocation(
            ToolbarItemLocation::Hidden,
        ));
        cx.notify();
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
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            regex: self.regex,
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

    fn toggle_option(&mut self, option: SearchOption, window: &mut Window, cx: &mut Context<Self>) {
        match option {
            SearchOption::CaseSensitive => self.case_sensitive = !self.case_sensitive,
            SearchOption::WholeWord => self.whole_word = !self.whole_word,
            SearchOption::Regex => self.regex = !self.regex,
        }
        self.run_search(window, cx);
    }

    fn toggle_replace_mode(&mut self, cx: &mut Context<Self>) {
        if self
            .active_item
            .as_ref()
            .is_some_and(|item| !item.supports_replace(cx))
        {
            return;
        }
        self.show_replace = !self.show_replace;
        if self.show_replace
            && let Some(replace_input) = &self.replace_input
        {
            replace_input.update(cx, |editor, cx| editor.set_text(&self.replacement, cx));
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
        window.focus(&handles[next]);
        cx.stop_propagation();
    }
}

impl Render for SearchBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }
        let colors = color::current(cx);
        // 按钮点击经弱引用更新组件状态。
        let weak = cx.weak_entity();
        let toggle_replace = {
            let weak = weak.clone();
            move |_: &gpui::ClickEvent, _window: &mut Window, cx: &mut App| {
                if let Some(search_bar) = weak.upgrade() {
                    search_bar.update(cx, |search_bar, cx| {
                        search_bar.toggle_replace_mode(cx);
                    });
                }
            }
        };
        let find_prev = {
            let weak = weak.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                if let Some(search_bar) = weak.upgrade() {
                    search_bar.update(cx, |search_bar, cx| {
                        search_bar.move_active(Direction::Prev, window, cx);
                    });
                }
            }
        };
        let find_next = {
            let weak = weak.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                if let Some(search_bar) = weak.upgrade() {
                    search_bar.update(cx, |search_bar, cx| {
                        search_bar.move_active(Direction::Next, window, cx);
                    });
                }
            }
        };
        let close = {
            let weak = weak.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                if let Some(search_bar) = weak.upgrade() {
                    search_bar.update(cx, |search_bar, cx| {
                        search_bar.close(window, cx);
                    });
                }
            }
        };
        let replace_next = {
            let weak = weak.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                if let Some(search_bar) = weak.upgrade() {
                    search_bar.update(cx, |search_bar, cx| {
                        search_bar.replace_next(window, cx);
                    });
                }
            }
        };
        let replace_all = {
            let weak = weak.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
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
        // 计数文案：无匹配时用占位色。
        let count_color = if match_count > 0 {
            colors.text_muted
        } else {
            colors.text_placeholder
        };
        let count_text = if match_count > 0 {
            format!(
                "{}/{}",
                active_match_index.map_or(0, |i| i + 1),
                match_count
            )
        } else {
            "0/0".to_string()
        };
        // 按持有焦点的输入框动态附加 in_replace 标签：
        // keymap 据此声明式区分查询/替换框的 Enter 语义。
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
            .mt(space::S6)
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
            // 搜索行：输入框容器（选项按钮内嵌右缘）+ 替换 toggle + 计数/跳转 + 关闭。
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S6)
                    // 输入框容器：边框包裹，选项按钮内嵌右侧。
                    .child(SearchInput::new(
                        "buffer-search-input",
                        self.query_input
                            .as_ref()
                            .unwrap()
                            .clone()
                            .into_any_element(),
                        Some(QueryOptions {
                            case_sensitive: self.case_sensitive,
                            whole_word: self.whole_word,
                            regex: self.regex,
                        }),
                    ))
                    // 替换模式 toggle（只读目标保留相同布局并显示禁用态）。
                    .child(
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
                    // 计数 + 上下跳转（ChevronLeft/Right）。
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(space::S4)
                            .child(
                                div()
                                    .text_color(count_color)
                                    .text_size(typography::ui() * 0.85)
                                    .child(count_text),
                            )
                            .child(
                                Button::icon("search-prev", "icons/chevron_left.svg")
                                    .label("上一个匹配")
                                    .shortcut(&FindPrevious, cx)
                                    .on_click(find_prev),
                            )
                            .child(
                                Button::icon("search-next", "icons/chevron_right.svg")
                                    .label("下一个匹配")
                                    .shortcut(&FindNext, cx)
                                    .on_click(find_next),
                            ),
                    )
                    // 关闭搜索条。
                    .child(
                        Button::icon("search-close", "icons/close.svg")
                            .label("关闭")
                            .shortcut(&ClearSearch, cx)
                            .on_click(close),
                    ),
            )
            .when(self.show_replace && supports_replace, |this| {
                // 替换行：替换输入框 + 替换 / 全部替换。
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S6)
                        .child(SearchInput::new(
                            "buffer-replace-input",
                            self.replace_input
                                .as_ref()
                                .unwrap()
                                .clone()
                                .into_any_element(),
                            None,
                        ))
                        .child(
                            Button::icon("search-replace-next", "icons/replace_next.svg")
                                .label("替换")
                                .shortcut(&ReplaceNext, cx)
                                .on_click(replace_next),
                        )
                        .child(
                            Button::icon("search-replace-all", "icons/replace_all.svg")
                                .label("全部替换")
                                .shortcut(&ReplaceAll, cx)
                                .on_click(replace_all),
                        ),
                )
            })
            .into_any_element()
    }
}

// ═══ SearchBar actions（keymap BufferSearchBar / ProjectSearchBar 上下文绑定）═══

impl SearchBar {
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_replace_mode(cx);
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
        self.toggle_option(SearchOption::CaseSensitive, window, cx);
    }

    fn handle_toggle_whole_word(
        &mut self,
        _: &ToggleWholeWord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_option(SearchOption::WholeWord, window, cx);
    }

    fn handle_toggle_regex(
        &mut self,
        _: &ToggleRegex,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_option(SearchOption::Regex, window, cx);
    }

    fn handle_tab(&mut self, _: &Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_focus(Direction::Next, window, cx);
    }

    fn handle_backtab(&mut self, _: &Backtab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_focus(Direction::Prev, window, cx);
    }
}
