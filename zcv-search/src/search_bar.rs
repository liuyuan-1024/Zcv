//! 共享搜索栏会话。
//!
//! 文件内搜索、项目搜索、差异搜索与提交图搜索共用同一套会话状态：
//! 查询/替换输入、匹配选项、搜索条可见性、替换开关、按键上下文与 action 接线、命中计数与导航，以及由 SearchableItemHandle 驱动的 search/clear。
//! 各宿主只保留工具项位置、目标解析、额外插槽与领域逻辑（如项目搜索的防抖、提交图的命中计算）。
//!
//! 目标在宿主解析后经 SearchBar::set_target 注入；搜索栏不反向依赖任何具体 Item。

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, KeyContext, ParentElement, Styled, Subscription,
    Window, div, prelude::*,
};
use zcv_actions::{
    Backtab, ClearSearch, FindNext, FindPrevious, ReplaceAll, ReplaceNext, SelectAll, Tab,
    ToggleCaseSensitive, ToggleRegex, ToggleReplace, ToggleWholeWord,
};
use zcv_editor::{Editor, EditorEvent, LanguageRegistry};
use zcv_project::SearchQuery;
use zcv_theme::{color, space};
use zcv_ui::{Button, MatchOption, MatchOptions, ReplaceInput, SearchInput};
use zcv_workspace::{Direction, SearchableItemHandle, WeakSearchableItemHandle};

/// 搜索栏配置：由宿主构造一次，定义该搜索入口的身份与能力。
pub struct SearchBarConfig {
    /// 元素 id 前缀；同屏多个搜索栏据此区分。
    pub id_prefix: &'static str,
    /// 键位上下文名（BufferSearchBar / ProjectSearchBar / ProjectDiffSearchBar / GitGraphSearchBar）。
    pub key_context: &'static str,
    /// 宿主是否允许替换；目标自身不支持替换时仍然禁用。
    pub supports_replace: bool,
    pub query_placeholder: &'static str,
    pub replace_placeholder: &'static str,
    /// 是否可关闭。
    ///
    /// 可关闭的搜索栏（文件内搜索、项目搜索）初始隐藏，由 deploy/close 控制显隐；
    /// 常驻搜索栏（差异、提交图）初始可见且不响应关闭，仅清空查询。
    pub dismissible: bool,
}

/// SearchBar::render 的宿主插槽。
///
/// 替换开关与替换行由搜索栏自行构建；这里只放宿主特有的控件。
#[derive(Default)]
pub struct SearchBarSlots {
    /// 搜索输入框左侧的宿主控件（如「折叠全部文件」）。
    pub leading: Option<AnyElement>,
    /// 命中计数与替换开关之后的宿主控件（如「重做全部」）。
    pub external: Vec<AnyElement>,
}

/// 共享搜索栏会话。
pub struct SearchBar {
    config: SearchBarConfig,
    visible: bool,
    show_replace: bool,
    options: MatchOptions,
    /// 当前搜索目标；由宿主在活动 Item 变化时注入。
    ///
    /// 保存弱句柄：自搜索视图（项目搜索、提交图）本身持有本搜索栏，强句柄会构成环，关闭标签/面板后两者都无法释放。
    target: Option<Box<dyn WeakSearchableItemHandle>>,
    target_subscription: Option<Subscription>,
    query_input: Entity<Editor>,
    replace_input: Entity<Editor>,
    input_subscriptions: Vec<Subscription>,
}

impl SearchBar {
    /// `language_registry` 由装配层注入：搜索输入框不自建注册表。
    pub fn new(
        config: SearchBarConfig,
        language_registry: std::sync::Arc<LanguageRegistry>,
        cx: &mut Context<Self>,
    ) -> Self {
        let query_registry = std::sync::Arc::clone(&language_registry);
        let query_input = cx.new(move |cx| Editor::auto_height(1, Some(4), query_registry, cx));
        let replace_input =
            cx.new(move |cx| Editor::auto_height(1, Some(4), language_registry, cx));
        query_input.update(cx, |editor, cx| {
            editor.set_placeholder_text(config.query_placeholder, cx)
        });
        replace_input.update(cx, |editor, cx| {
            editor.set_placeholder_text(config.replace_placeholder, cx)
        });
        Self {
            visible: !config.dismissible,
            config,
            show_replace: false,
            options: MatchOptions::default(),
            target: None,
            target_subscription: None,
            query_input,
            replace_input,
            input_subscriptions: Vec::new(),
        }
    }

    pub fn visible(&self) -> bool {
        self.visible
    }

    pub fn options(&self) -> MatchOptions {
        self.options
    }

    pub fn query_text(&self, cx: &App) -> String {
        self.query_input.read(cx).text(cx)
    }

    /// 查询输入框的焦点句柄。
    pub fn query_focus_handle(&self, cx: &App) -> FocusHandle {
        self.query_input.read(cx).focus_handle()
    }

    /// 把键盘焦点交给查询输入框。
    pub fn focus_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.query_input.read(cx).focus_handle(), cx);
    }

    /// 升级当前搜索目标；目标已释放时返回 None。
    fn search_target(&self) -> Option<Box<dyn SearchableItemHandle>> {
        self.target.as_ref()?.upgrade()
    }

    /// 是否支持替换：宿主配置与目标能力同时成立才启用。
    pub fn supports_replace(&self, cx: &App) -> bool {
        self.config.supports_replace
            && self
                .search_target()
                .is_some_and(|item| item.supports_replace(cx))
    }

    /// 由宿主注入/更换搜索目标；订阅目标的搜索事件以刷新计数与高亮。
    ///
    /// 目标以弱句柄保存，不延长目标生命周期：自搜索视图（项目搜索、提交图）本身就持有本搜索栏，目标释放后升级失败，搜索/导航自然成为空操作。
    pub fn set_target(
        &mut self,
        target: Option<Box<dyn WeakSearchableItemHandle>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ensure_input_subscription(window, cx);
        self.target_subscription = None;
        let item_changed = match (&self.target, &target) {
            (Some(old), Some(new)) => old.id() != new.id(),
            (None, None) => false,
            _ => true,
        };
        if let Some(item) = target.as_ref().and_then(|target| target.upgrade()) {
            let weak = cx.weak_entity();
            self.target_subscription = Some(item.subscribe_to_search_events(
                window,
                cx,
                Box::new(move |_, _window, cx| {
                    if let Some(bar) = weak.upgrade() {
                        bar.update(cx, |_, cx| cx.notify());
                    }
                }),
            ));
        }
        self.target = target;
        if !self.supports_replace(cx) {
            self.show_replace = false;
        }
        if item_changed && self.visible {
            self.run_search(window, cx);
        }
        cx.notify();
    }

    /// 恢复持久化状态：写入查询与匹配选项（不触发聚焦；随后由宿主注入目标并重搜）。
    pub fn restore(&mut self, query: &str, options: MatchOptions, cx: &mut Context<Self>) {
        self.options = options;
        self.query_input
            .update(cx, |editor, cx| editor.set_text(query, cx));
    }

    /// 部署搜索条：无论当前状态，一律打开并把焦点移到搜索框。
    ///
    /// query_seed 为调用方预先提取的建议（项目搜索必须在切换活动 Item 前提取）；
    /// 无种子时向目标请求查询建议（选区文本）。cmd-f 只开不关；关闭由 esc 或关闭入口触发。
    pub fn deploy(
        &mut self,
        query_seed: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_visible = self.visible;
        self.visible = true;
        self.ensure_input_subscription(window, cx);
        let seed = query_seed.or_else(|| {
            self.search_target()
                .and_then(|item| item.query_suggestion(cx))
        });
        let seeded = seed.is_some();
        if let Some(seed) = seed {
            // 正则模式下先转义原始文本，避免选区中的元字符改变查询语义。
            let seed = if self.options.regex {
                regex::escape(&seed)
            } else {
                seed
            };
            self.query_input
                .update(cx, |editor, cx| editor.set_text(&seed, cx));
        }
        window.focus(&self.query_input.read(cx).focus_handle(), cx);
        // 全选查询文本：直接击键即可整体替换。
        window.dispatch_action(Box::new(SelectAll), cx);
        if !was_visible || seeded {
            self.run_search(window, cx);
        }
        cx.notify();
    }

    /// 关闭搜索条：清空搜索状态并把焦点还给目标。
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.visible = false;
        if let Some(target) = self.search_target() {
            target.clear_search(window, cx);
            window.focus(&target.item_focus_handle(cx), cx);
        }
        cx.notify();
    }

    /// 清空搜索：可关闭搜索栏同时关闭，常驻搜索栏保留可见性。
    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.config.dismissible {
            self.close(window, cx);
        } else if let Some(target) = self.search_target() {
            target.clear_search(window, cx);
            cx.notify();
        }
    }

    pub fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.visible {
            self.close(window, cx);
        } else {
            self.deploy(None, window, cx);
        }
    }

    /// 用当前查询与选项在目标上执行搜索；计数由渲染时读取目标状态。
    fn run_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.search_target() else {
            return;
        };
        let query = SearchQuery {
            query: self.query_input.read(cx).text(cx),
            case_sensitive: self.options.case_sensitive,
            whole_word: self.options.whole_word,
            regex: self.options.regex,
        };
        target.search(&query, window, cx);
        cx.notify();
    }

    fn search_count(&self, cx: &App) -> (usize, Option<usize>) {
        self.search_target()
            .map_or((0, None), |item| item.search_count(cx))
    }

    fn move_active(&mut self, direction: Direction, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(target) = self.search_target() {
            target.activate_match_in_direction(direction, 1, window, cx);
        }
        cx.notify();
    }

    fn toggle_option(&mut self, option: MatchOption, window: &mut Window, cx: &mut Context<Self>) {
        self.options = self.options.toggled(option);
        self.run_search(window, cx);
    }

    fn toggle_replace_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.supports_replace(cx) {
            return;
        }
        let closing = self.show_replace;
        self.show_replace = !self.show_replace;
        // 打开替换行时默认聚焦替换输入框。
        if self.show_replace {
            window.focus(&self.replace_input.read(cx).focus_handle(), cx);
        }
        // 收起替换行时把焦点还给搜索输入框。
        if closing {
            window.focus(&self.query_input.read(cx).focus_handle(), cx);
        }
        cx.notify();
    }

    fn replace_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let replacement = self.replace_input.read(cx).text(cx);
        if let Some(target) = self.search_target()
            && target.replace_current(&replacement, window, cx)
        {
            // 替换触发编辑 → 目标侧重搜并 emit；这里跟随活动匹配前移一位。
            self.move_active(Direction::Next, window, cx);
        }
    }

    fn replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let replacement = self.replace_input.read(cx).text(cx);
        if let Some(target) = self.search_target() {
            target.replace_all(&replacement, window, cx);
        }
    }

    /// 焦点在查询输入框 → 替换输入框 → 搜索目标间循环。
    fn cycle_focus(&mut self, direction: Direction, window: &mut Window, cx: &mut Context<Self>) {
        let mut handles = vec![self.query_input.read(cx).focus_handle()];
        if self.show_replace {
            handles.push(self.replace_input.read(cx).focus_handle());
        }
        if let Some(target) = self.search_target() {
            handles.push(target.item_focus_handle(cx));
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

    /// 懒建立查询输入订阅（首次拿到 window 时）；输入变化自动重搜。
    fn ensure_input_subscription(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.input_subscriptions.is_empty() {
            return;
        }
        let weak = cx.weak_entity();
        self.input_subscriptions
            .push(window.subscribe(&self.query_input, cx, {
                move |_, event: &EditorEvent, window, cx| {
                    if !matches!(event, EditorEvent::Edited { .. }) {
                        return;
                    }
                    if let Some(bar) = weak.upgrade() {
                        bar.update(cx, |bar, cx| bar.run_search(window, cx));
                    }
                }
            }));
    }

    /// 渲染搜索栏；宿主插槽之外的部分（替换开关、替换行、按键接线）由搜索栏统一构建。
    pub fn render(
        &mut self,
        slots: SearchBarSlots,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.visible {
            return div().into_any_element();
        }
        let colors = color::current(cx);
        let SearchBarSlots { leading, external } = slots;
        let weak = cx.weak_entity();
        let supports_replace = self.supports_replace(cx);
        let (match_count, active_match_index) = self.search_count(cx);

        let replace_toggle = weak.clone();
        let on_toggle_replace = move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
            if let Some(bar) = replace_toggle.upgrade() {
                bar.update(cx, |bar, cx| bar.toggle_replace_mode(window, cx));
            }
        };
        let previous = weak.clone();
        let on_previous = move |window: &mut Window, cx: &mut App| {
            if let Some(bar) = previous.upgrade() {
                bar.update(cx, |bar, cx| bar.move_active(Direction::Prev, window, cx));
            }
        };
        let next = weak.clone();
        let on_next = move |window: &mut Window, cx: &mut App| {
            if let Some(bar) = next.upgrade() {
                bar.update(cx, |bar, cx| bar.move_active(Direction::Next, window, cx));
            }
        };
        let toggle_option = weak.clone();
        let on_toggle = move |option: MatchOption, window: &mut Window, cx: &mut App| {
            if let Some(bar) = toggle_option.upgrade() {
                bar.update(cx, |bar, cx| bar.toggle_option(option, window, cx));
            }
        };
        let replace = weak.clone();
        let on_replace = move |window: &mut Window, cx: &mut App| {
            if let Some(bar) = replace.upgrade() {
                bar.update(cx, |bar, cx| bar.replace_next(window, cx));
            }
        };
        let replace_all = weak.clone();
        let on_replace_all = move |window: &mut Window, cx: &mut App| {
            if let Some(bar) = replace_all.upgrade() {
                bar.update(cx, |bar, cx| bar.replace_all(window, cx));
            }
        };

        let mut search_input = SearchInput::new(
            self.config.id_prefix,
            self.query_input.clone().into_any_element(),
        )
        .shortcut_resolver(zcv_keymap::display_shortcut)
        .options(self.options)
        .on_toggle(on_toggle)
        .count(active_match_index, match_count)
        .on_previous(on_previous)
        .on_next(on_next);
        if supports_replace {
            search_input = search_input.external(
                Button::icon(
                    format!("{}-toggle-replace", self.config.id_prefix),
                    "icons/replace.svg",
                )
                .label("替换")
                .shortcut(zcv_keymap::display_shortcut(&ToggleReplace, cx))
                .color(if self.show_replace {
                    colors.icon_accent
                } else {
                    colors.text_muted
                })
                .on_click(on_toggle_replace),
            );
        }
        for element in external {
            search_input = search_input.external(element);
        }
        let row = div()
            .w_full()
            .flex()
            .items_center()
            .gap(space::S6)
            .when_some(leading, |row, leading| row.child(leading))
            .child(div().flex_1().min_w_0().child(search_input));

        let replacement_line = (supports_replace && self.show_replace).then(|| {
            ReplaceInput::new(
                format!("{}-replace", self.config.id_prefix),
                self.replace_input.clone().into_any_element(),
            )
            .shortcut_resolver(zcv_keymap::display_shortcut)
            .on_replace(on_replace)
            .on_replace_all(on_replace_all)
            .into_any_element()
        });

        // 替换类快捷键仅在替换输入框聚焦时生效：按焦点动态附加 in_replace 标签，keymap 据此区分查询框与替换框。
        let mut key_context = KeyContext::new_with_defaults();
        key_context.add(self.config.key_context);
        if self
            .replace_input
            .read(cx)
            .focus_handle()
            .is_focused(window)
        {
            key_context.add("in_replace");
        }

        div()
            .w_full()
            .key_context(key_context)
            .flex()
            .flex_col()
            .gap(space::S6)
            .on_action(cx.listener(Self::handle_find_next))
            .on_action(cx.listener(Self::handle_find_previous))
            .on_action(cx.listener(Self::handle_toggle_replace))
            .on_action(cx.listener(Self::handle_replace_next))
            .on_action(cx.listener(Self::handle_replace_all))
            .when(self.config.dismissible, |this| {
                this.on_action(cx.listener(Self::handle_clear_search))
            })
            .on_action(cx.listener(Self::handle_toggle_case_sensitive))
            .on_action(cx.listener(Self::handle_toggle_whole_word))
            .on_action(cx.listener(Self::handle_toggle_regex))
            .on_action(cx.listener(Self::handle_tab))
            .on_action(cx.listener(Self::handle_backtab))
            .child(row)
            .when_some(replacement_line, |this, line| this.child(line))
            .into_any_element()
    }
}

// ═══ SearchBar actions（keymap 搜索上下文绑定）═══

impl SearchBar {
    /// 视图体聚焦时由宿主转发的匹配导航。
    pub fn find_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.move_active(Direction::Next, window, cx);
    }

    /// 视图体聚焦时由宿主转发的匹配导航。
    pub fn find_previous(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.move_active(Direction::Prev, window, cx);
    }

    /// 视图体聚焦时由宿主转发的匹配选项切换。
    pub fn toggle_case_sensitive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_option(MatchOption::CaseSensitive, window, cx);
    }

    /// 视图体聚焦时由宿主转发的匹配选项切换。
    pub fn toggle_whole_word(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_option(MatchOption::WholeWord, window, cx);
    }

    /// 视图体聚焦时由宿主转发的匹配选项切换。
    pub fn toggle_regex(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_option(MatchOption::Regex, window, cx);
    }

    fn handle_find_next(&mut self, _: &FindNext, window: &mut Window, cx: &mut Context<Self>) {
        self.find_next(window, cx);
    }

    fn handle_find_previous(
        &mut self,
        _: &FindPrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.find_previous(window, cx);
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
        self.clear(window, cx);
    }

    fn handle_toggle_case_sensitive(
        &mut self,
        _: &ToggleCaseSensitive,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_case_sensitive(window, cx);
    }

    fn handle_toggle_whole_word(
        &mut self,
        _: &ToggleWholeWord,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_whole_word(window, cx);
    }

    fn handle_toggle_regex(
        &mut self,
        _: &ToggleRegex,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_regex(window, cx);
    }

    fn handle_tab(&mut self, _: &Tab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_focus(Direction::Next, window, cx);
    }

    fn handle_backtab(&mut self, _: &Backtab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_focus(Direction::Prev, window, cx);
    }
}
