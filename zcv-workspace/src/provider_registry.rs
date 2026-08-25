//! Provider 注册表原语：按具体类型去重注册、逆序优先查找。
//!
//! item_provider（文件 Item 工厂）与 preview（格式预览工厂）共用同一套注册/查找语义，此处收敛为单一实现，消费方只保留 trait 与包装函数。

use std::any::TypeId;
use std::sync::Arc;

use gpui::{App, BorrowAppContext, Global};

/// 注册表中的一条记录：具体 Provider 类型 + trait object 形式的实例。
pub(crate) struct RegisteredProvider<T: ?Sized> {
    pub(crate) type_id: TypeId,
    pub(crate) provider: Arc<T>,
}

/// 全局 Provider 注册表；同一具体类型只保留一次注册。
pub(crate) struct ProviderRegistry<T: ?Sized> {
    pub(crate) providers: Vec<RegisteredProvider<T>>,
}

impl<T: ?Sized> Default for ProviderRegistry<T> {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
        }
    }
}

impl<T: ?Sized + 'static> Global for ProviderRegistry<T> {}

impl<T: ?Sized + 'static> ProviderRegistry<T> {
    /// 注册 Provider；同一具体类型重复注册时忽略。
    ///
    /// `type_id` 必须是具体 Provider 类型的 `TypeId`（而非 trait object 的），由调用方在把实例上转为 `Arc<T>` 时同步提供。
    pub(crate) fn register(provider: Arc<T>, type_id: TypeId, cx: &mut App) {
        if !cx.has_global::<ProviderRegistry<T>>() {
            cx.set_global(ProviderRegistry::<T>::default());
        }
        cx.update_global::<ProviderRegistry<T>, _>(|registry, _| {
            if registry
                .providers
                .iter()
                .all(|entry| entry.type_id != type_id)
            {
                registry
                    .providers
                    .push(RegisteredProvider { type_id, provider });
            }
        });
    }

    /// 返回最后注册且满足条件的 Provider。
    pub(crate) fn find(cx: &App, mut predicate: impl FnMut(&T) -> bool) -> Option<Arc<T>> {
        cx.try_global::<ProviderRegistry<T>>()?
            .providers
            .iter()
            .rev()
            .find(|entry| predicate(&*entry.provider))
            .map(|entry| Arc::clone(&entry.provider))
    }
}
