//! 文件内搜索与项目搜索共享的搜索会话控制器及界面。
//!
//! 持有查询、选项、替换文本、可见性和输入焦点等会话状态，把用户操作组装成 [`SearchQuery`] 并派发给当前 Item 的 [`SearchableItemHandle`]；
//! 匹配与结果数据仍由具体 Item 持有。
//! Pane/Workspace 接线位于 `buffer_search`，跨文件搜索执行位于 `project_search`。

use std::sync::Arc;

use gpui::{
    AnyElement, App, Component, Context, IntoElement, ParentElement, Render, RenderOnce,
    SharedString, Styled, Window, div, prelude::*, px,
};
use zcv_actions::{
    Backtab, ClearSearch, FindNext, FindPrevious, ReplaceAll, ReplaceNext, Tab,
    ToggleCaseSensitive, ToggleRegex, ToggleReplace, ToggleWholeWord,
};
use zcv_text::SearchQuery;
use zcv_theme::{color, space, typography};
use zcv_ui::{ErasedEditor, Glyph};

use zcv_workspace::{
    Direction, ItemHandle, SearchableItemHandle, ToolbarItemEvent, ToolbarItemLocation,
};

#[derive(Clone, Copy)]
enum SearchOption {
    CaseSensitive,
    WholeWord,
    Regex,
}

/// SearchBar 内部唯一的查询输入外观。
struct SearchInput {
    id: SharedString,
    input: AnyElement,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
}

impl SearchInput {
    fn new(
        id: impl Into<SharedString>,
        input: AnyElement,
        case_sensitive: bool,
        whole_word: bool,
        regex: bool,
    ) -> Self {
        Self {
            id: id.into(),
            input,
            case_sensitive,
            whole_word,
            regex,
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
            .h(px(26.))
            .px(space::S6)
            .rounded(px(4.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.background)
            .child(self.input)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        Glyph::icon(case_id, "icons/case_sensitive.svg")
                            .label("区分大小写")
                            .shortcut(&ToggleCaseSensitive, cx)
                            .color(if self.case_sensitive {
                                colors.icon_accent
                            } else {
                                colors.text_muted
                            })
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ToggleCaseSensitive), cx)
                            }),
                    )
                    .child(
                        Glyph::icon(word_id, "icons/whole_word.svg")
                            .label("整词匹配")
                            .shortcut(&ToggleWholeWord, cx)
                            .color(if self.whole_word {
                                colors.icon_accent
                            } else {
                                colors.text_muted
                            })
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ToggleWholeWord), cx)
                            }),
                    )
                    .child(
                        Glyph::icon(regex_id, "icons/regex.svg")
                            .label("正则表达式")
                            .shortcut(&ToggleRegex, cx)
                            .color(if self.regex {
                                colors.icon_accent
                            } else {
                                colors.text_muted
                            })
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(ToggleRegex), cx)
                            }),
                    ),
            )
    }
}

pub(crate) struct SearchBar {
    visible: bool,
    show_replace: bool,
    query: String,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    replacement: String,
    /// 当前搜索目标（pane 的活动 item 的可搜索句柄）。
    active_item: Option<Box<dyn SearchableItemHandle>>,
    query_input: Option<Arc<dyn ErasedEditor>>,
    replace_input: Option<Arc<dyn ErasedEditor>>,
    input_subscriptions: Vec<gpui::Subscription>,
    active_item_subscription: Option<gpui::Subscription>,
}

impl gpui::EventEmitter<ToolbarItemEvent> for SearchBar {}

impl SearchBar {
    pub(crate) fn new(_cx: &mut Context<Self>) -> Self {
        // 输入框懒创建（首次打开搜索条时）：ErasedEditor 的创建与订阅都需要 window，且避免在无装配（如 Pane 单测）环境下构造。
        Self {
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
        // 工厂未初始化（无装配环境，如 Pane 单测）时输入框缺席，搜索条降级为只显示按钮。
        let Some(factory) = zcv_ui::EDITOR_FACTORY.get() else {
            return;
        };
        let query_input = factory(cx);
        let replace_input = factory(cx);
        query_input.set_placeholder_text("搜索...", cx);
        replace_input.set_placeholder_text("替换为...", cx);
        let weak = cx.weak_entity();
        self.input_subscriptions.push(query_input.subscribe(
            Box::new({
                let weak = weak.clone();
                move |_, window, cx| {
                    if let Some(search_bar) = weak.upgrade() {
                        search_bar.update(cx, |search_bar, cx| {
                            search_bar.run_search(window, cx);
                        });
                    }
                }
            }),
            window,
            cx,
        ));
        self.input_subscriptions.push(replace_input.subscribe(
            Box::new({
                let weak = weak.clone();
                move |_, _window, cx| {
                    if let Some(search_bar) = weak.upgrade() {
                        search_bar.update(cx, |search_bar, cx| {
                            search_bar.replacement = search_bar
                                .replace_input
                                .as_ref()
                                .map_or(String::new(), |input| input.text(cx));
                        });
                    }
                }
            }),
            window,
            cx,
        ));
        self.query_input = Some(query_input);
        self.replace_input = Some(replace_input);
    }

    /// 部署搜索条（cmd-f / 工具栏按钮）：无论当前状态，一律打开并把焦点移到搜索框；
    /// 关闭只由 esc / ✕ 触发（对齐 Zed：cmd-f 永不关闭搜索条）。
    pub(super) fn deploy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let was_visible = self.visible;
        self.visible = true;
        self.ensure_inputs(window, cx);
        let query_input = self.query_input.as_ref().unwrap();
        query_input.set_text(&self.query, cx);
        window.focus(&query_input.focus_handle(cx));
        if !was_visible {
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

    /// 关闭搜索条（esc / ✕）：清空搜索状态并把焦点还给活动 item（对齐 Zed 的 dismiss）。
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
            .map_or(String::new(), |input| input.text(cx));
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
            replace_input.set_text(&self.replacement, cx);
        }
        cx.notify();
    }

    fn replace_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.replacement = self
            .replace_input
            .as_ref()
            .map_or(String::new(), |input| input.text(cx));
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
            .map_or(String::new(), |input| input.text(cx));
        if let Some(item) = &self.active_item {
            item.replace_all(&self.replacement, window, cx);
        }
    }

    /// 焦点在 query 输入框 → 替换输入框 → 活动 item 间循环（对齐 Zed 的 cycle_field）。
    fn cycle_focus(&mut self, direction: Direction, window: &mut Window, cx: &mut Context<Self>) {
        let mut handles = vec![self.query_input.as_ref().unwrap().focus_handle(cx)];
        if self.show_replace {
            handles.push(self.replace_input.as_ref().unwrap().focus_handle(cx));
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.visible {
            return div().into_any_element();
        }
        let colors = color::current(cx);
        // 按钮点击经弱引用更新组件状态（对齐 zcv 现有 on_click 模式）。
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
        // 计数文案：无匹配时用占位色（对齐 Zed 的错误态计数）。
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
        div()
            .key_context("SearchBar")
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
                    // 输入框容器：边框包裹，选项按钮内嵌右侧（对齐 Zed 的 input_style 结构）。
                    .child(SearchInput::new(
                        "buffer-search-input",
                        self.query_input.as_ref().unwrap().render(),
                        self.case_sensitive,
                        self.whole_word,
                        self.regex,
                    ))
                    // 替换模式 toggle（对齐 Zed 的 Replace 图标按钮；只读目标保留相同布局并显示禁用态）。
                    .child(
                        Glyph::icon("search-toggle-replace", "icons/replace.svg")
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
                    // 计数 + 上下跳转（对齐 Zed 的 matches_column：ChevronLeft/Right）。
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_color(count_color)
                                    .text_size(typography::ui() * 0.85)
                                    .child(count_text),
                            )
                            .child(
                                Glyph::icon("search-prev", "icons/chevron_left.svg")
                                    .label("上一个匹配")
                                    .shortcut(&FindPrevious, cx)
                                    .on_click(find_prev),
                            )
                            .child(
                                Glyph::icon("search-next", "icons/chevron_right.svg")
                                    .label("下一个匹配")
                                    .shortcut(&FindNext, cx)
                                    .on_click(find_next),
                            ),
                    )
                    // 关闭搜索条。
                    .child(
                        Glyph::icon("search-close", "icons/close.svg")
                            .label("关闭")
                            .shortcut(&ClearSearch, cx)
                            .on_click(close),
                    ),
            )
            .when(self.show_replace && supports_replace, |this| {
                // 替换行：替换输入框 + 替换 / 全部替换（对齐 Zed 的替换栏）。
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S6)
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .h(px(26.))
                                .px(space::S6)
                                .rounded(px(4.))
                                .border_1()
                                .border_color(colors.border)
                                .bg(colors.background)
                                .child(self.replace_input.as_ref().unwrap().render()),
                        )
                        .child(
                            Glyph::icon("search-replace-next", "icons/replace_next.svg")
                                .label("替换")
                                .shortcut(&ReplaceNext, cx)
                                .on_click(replace_next),
                        )
                        .child(
                            Glyph::icon("search-replace-all", "icons/replace_all.svg")
                                .label("全部替换")
                                .shortcut(&ReplaceAll, cx)
                                .on_click(replace_all),
                        ),
                )
            })
            .into_any_element()
    }
}

// ═══ SearchBar actions（keymap "SearchBar" 上下文绑定）═══

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
