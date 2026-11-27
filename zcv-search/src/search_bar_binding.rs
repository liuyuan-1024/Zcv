//! 工具栏当前呈现的搜索会话绑定；查询状态与目标订阅仍由 SearchBar 拥有。

use gpui::{App, Entity, Window};
use zcv_workspace::WeakSearchableItemHandle;

use crate::SearchBar;

/// 随工具栏创建和销毁，集中管理活动搜索会话的切换与解绑。
///
/// 视图持有各自的搜索会话，工具栏只保留当前呈现的会话句柄。
/// 目标解析由宿主负责，绑定只接收弱目标，不延长目标生命周期。
#[derive(Default)]
pub struct SearchBarBinding {
    active_bar: Option<Entity<SearchBar>>,
}

impl SearchBarBinding {
    pub fn active_bar(&self) -> Option<&Entity<SearchBar>> {
        self.active_bar.as_ref()
    }

    /// 工具栏隐藏或释放时结束绑定，搜索会话仍由其所属视图持有。
    pub fn clear(&mut self, cx: &mut App) {
        if let Some(previous) = self.active_bar.take() {
            previous.update(cx, |bar, cx| bar.clear_target(cx));
        }
    }

    /// 切换会话前解绑旧目标；传入 None 表示工具栏不再呈现搜索会话。
    pub fn set_active(
        &mut self,
        active: Option<(Entity<SearchBar>, Box<dyn WeakSearchableItemHandle>)>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let next_id = active.as_ref().map(|(bar, _)| bar.entity_id());
        if self.active_bar.as_ref().map(Entity::entity_id) != next_id {
            self.clear(cx);
        }
        if let Some((bar, target)) = active {
            bar.update(cx, |bar, cx| bar.set_target(Some(target), window, cx));
            self.active_bar = Some(bar);
        }
    }
}
