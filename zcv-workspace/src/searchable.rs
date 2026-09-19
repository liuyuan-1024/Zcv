//! SearchableItem —— 可在自身内容中搜索 / 替换的 Item 契约。
//!
//! 搜索条只面向此 trait 编程，Editor 等 Item 提供搜索执行与匹配跳转。

use gpui::{App, Context, Entity, EntityId, EventEmitter, Subscription, WeakEntity, Window};
use zcv_project::SearchQuery;

use crate::item::{Item, ItemHandle};

/// 搜索状态变化通知（搜索条订阅以刷新计数与高亮）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchEvent {
    /// 匹配集合变化（新结果、清空或编辑后重搜）。
    MatchesInvalidated,
    /// 活动匹配序号变化。
    ActiveMatchChanged,
}

/// 类型擦除后的搜索事件回调。
pub(crate) type SearchEventHandler = Box<dyn Fn(&SearchEvent, &mut Window, &mut App) + Send>;

/// 跳转方向。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Direction {
    Prev,
    Next,
}

/// 可在自身内容中搜索的 Item。
///
/// 实现方持有匹配结果与活动位置；搜索会话控制器通过本协议派发统一的
/// [`zcv_project::SearchQuery`]。
pub trait SearchableItem: Item + EventEmitter<SearchEvent> {
    /// 是否支持替换。项目搜索等只读搜索目标返回 false，SearchBar 据此禁用替换入口。
    fn supports_replace(&self) -> bool {
        true
    }

    /// 部署搜索条时的查询建议（Editor 用主选区文本种入）；返回 None 表示无可种入的查询。
    fn query_suggestion(&self, _cx: &App) -> Option<String> {
        None
    }

    /// 执行搜索。同步执行：搜索是内存内匹配，单文件代价可控。
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

/// SearchableItem 的类型擦除句柄。
pub trait SearchableItemHandle: ItemHandle {
    /// 生成指向同一目标的弱句柄。
    ///
    /// 搜索目标可能是搜索栏宿主自身，搜索栏若持有强句柄就与宿主构成环；
    /// 因此搜索栏一律保存弱句柄。
    fn downgrade(&self) -> Box<dyn WeakSearchableItemHandle>;
    fn boxed_clone(&self) -> Box<dyn SearchableItemHandle>;
    fn subscribe_to_search_events(
        &self,
        window: &mut Window,
        cx: &mut App,
        handler: SearchEventHandler,
    ) -> Subscription;
    fn search(&self, query: &SearchQuery, window: &mut Window, cx: &mut App);
    fn clear_search(&self, window: &mut Window, cx: &mut App);
    fn search_count(&self, cx: &App) -> (usize, Option<usize>);
    fn supports_replace(&self, cx: &App) -> bool;
    fn query_suggestion(&self, cx: &App) -> Option<String>;
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
    fn downgrade(&self) -> Box<dyn WeakSearchableItemHandle> {
        Box::new(Entity::downgrade(self))
    }

    fn boxed_clone(&self) -> Box<dyn SearchableItemHandle> {
        Box::new(self.clone())
    }

    fn subscribe_to_search_events(
        &self,
        window: &mut Window,
        cx: &mut App,
        handler: SearchEventHandler,
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

    fn supports_replace(&self, cx: &App) -> bool {
        self.read(cx).supports_replace()
    }

    fn query_suggestion(&self, cx: &App) -> Option<String> {
        self.read(cx).query_suggestion(cx)
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

/// 搜索目标的类型擦除弱句柄。
///
/// 与 [`SearchableItemHandle`] 相对：它不延长目标生命周期。
/// 目标释放后升级失败，依赖目标的搜索操作自然成为空操作。
pub trait WeakSearchableItemHandle: Send + Sync {
    /// 目标实体 id；用于判断目标是否被更换。
    fn id(&self) -> EntityId;

    /// 升级为目标强句柄；目标已释放时返回 None。
    fn upgrade(&self) -> Option<Box<dyn SearchableItemHandle>>;
}

impl<T: SearchableItem> WeakSearchableItemHandle for WeakEntity<T> {
    fn id(&self) -> EntityId {
        self.entity_id()
    }

    fn upgrade(&self) -> Option<Box<dyn SearchableItemHandle>> {
        Some(Box::new(WeakEntity::upgrade(self)?))
    }
}
