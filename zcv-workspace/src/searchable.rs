//! SearchableItem —— 可在自身内容中搜索 / 替换的 Item 契约。
//!
//! 对齐 Zed 的 `workspace::searchable`：搜索条只面向此 trait 编程，Editor 等 Item 提供搜索执行与匹配跳转。

use gpui::{App, Context, Entity, EventEmitter, Subscription, Window};

use crate::item::{Item, ItemHandle};

/// 搜索状态变化通知（搜索条订阅以刷新计数与高亮）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchEvent {
    /// 匹配集合变化（新结果、清空或编辑后重搜）。
    MatchesInvalidated,
    /// 活动匹配序号变化。
    ActiveMatchChanged,
}

/// 跳转方向。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Direction {
    Prev,
    Next,
}

impl Direction {
    pub fn opposite(self) -> Self {
        match self {
            Direction::Prev => Direction::Next,
            Direction::Next => Direction::Prev,
        }
    }
}

/// 宿主层搜索参数（对齐 Zed 的 `project::search::SearchQuery`；
/// zcv 只保留文件内搜索所需字段，regex 选项直接映射到引擎的 `RegexSearchOptions`）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchQuery {
    pub query: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

/// 可在自身内容中搜索的 Item。
///
/// 实现方持有搜索状态（结果绑定 BufferVersion，编辑后自动重搜），搜索条只做UI 壳：把用户输入转成 [`SearchQuery`]，经 handle 调用本 trait 的方法。
pub trait SearchableItem: Item + EventEmitter<SearchEvent> {
    /// 执行搜索。同步执行：引擎搜索是内存内匹配，单文件代价可控。
    /// 结果由实现方持有（绑定 BufferVersion，编辑后自动重搜），搜索条经 `search_count` 读取计数。
    fn search(&mut self, query: &SearchQuery, window: &mut Window, cx: &mut Context<Self>);

    /// 清空搜索状态（清除高亮与活动匹配）。
    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>);

    /// 当前匹配总数与活动匹配序号（搜索条计数 "n/m" 用）。
    fn search_count(&self, cx: &App) -> (usize, Option<usize>);

    /// 按方向从活动匹配移动 `count` 步（循环），并激活目标匹配。
    fn activate_match_in_direction(
        &mut self,
        direction: Direction,
        count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    /// 替换活动匹配；返回是否实际替换。
    fn replace_current(
        &mut self,
        replacement: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool;

    /// 替换全部匹配；返回替换数量。
    fn replace_all(
        &mut self,
        replacement: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize;
}

/// SearchableItem 的类型擦除句柄（对齐 `PreviewItemHandle` 的桥接模式）。
pub trait SearchableItemHandle: ItemHandle {
    fn boxed_clone(&self) -> Box<dyn SearchableItemHandle>;
    fn subscribe_to_search_events(
        &self,
        window: &mut Window,
        cx: &mut App,
        handler: Box<dyn Fn(&SearchEvent, &mut Window, &mut App) + Send>,
    ) -> Subscription;
    fn search(&self, query: &SearchQuery, window: &mut Window, cx: &mut App);
    fn clear_search(&self, window: &mut Window, cx: &mut App);
    fn search_count(&self, cx: &App) -> (usize, Option<usize>);
    fn activate_match_in_direction(
        &self,
        direction: Direction,
        count: usize,
        window: &mut Window,
        cx: &mut App,
    );
    fn replace_current(&self, replacement: &str, window: &mut Window, cx: &mut App) -> bool;
    fn replace_all(&self, replacement: &str, window: &mut Window, cx: &mut App) -> usize;
}

impl<T: SearchableItem> SearchableItemHandle for Entity<T> {
    fn boxed_clone(&self) -> Box<dyn SearchableItemHandle> {
        Box::new(self.clone())
    }

    fn subscribe_to_search_events(
        &self,
        window: &mut Window,
        cx: &mut App,
        handler: Box<dyn Fn(&SearchEvent, &mut Window, &mut App) + Send>,
    ) -> Subscription {
        window.subscribe(self, cx, move |_, event: &SearchEvent, window, cx| {
            handler(event, window, cx)
        })
    }

    fn search(&self, query: &SearchQuery, window: &mut Window, cx: &mut App) {
        self.update(cx, |item, cx| item.search(query, window, cx));
    }

    fn clear_search(&self, window: &mut Window, cx: &mut App) {
        self.update(cx, |item, cx| item.clear_search(window, cx));
    }

    fn search_count(&self, cx: &App) -> (usize, Option<usize>) {
        self.read(cx).search_count(cx)
    }

    fn activate_match_in_direction(
        &self,
        direction: Direction,
        count: usize,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.update(cx, |item, cx| {
            item.activate_match_in_direction(direction, count, window, cx)
        });
    }

    fn replace_current(&self, replacement: &str, window: &mut Window, cx: &mut App) -> bool {
        self.update(cx, |item, cx| item.replace_current(replacement, window, cx))
    }

    fn replace_all(&self, replacement: &str, window: &mut Window, cx: &mut App) -> usize {
        self.update(cx, |item, cx| item.replace_all(replacement, window, cx))
    }
}
