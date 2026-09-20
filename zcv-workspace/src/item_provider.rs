//! ItemProvider 注册表：文件路径 → Workspace Item 的工厂。
//!
//! 具体文件类型（如文本编辑器）的 Item 构造由各 crate 注册，框架打开文件时经注册表分发，不依赖具体视图类型。

use std::any::TypeId;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, Context, Entity, Task, Window};
use zcv_project::Project;

use crate::Workspace;
use crate::item::ItemHandle;
use crate::provider_registry::ProviderRegistry;

/// 文件路径 → Workspace Item 的工厂接口。
pub trait ItemProvider: Send + Sync + 'static {
    fn supports(&self, path: &Path, cx: &App) -> bool;

    /// 为路径构造 Item；项目 buffer 打开等异步工作由 provider 自行完成。
    fn open_item(
        &self,
        path: PathBuf,
        project: Entity<Project>,
        cx: &mut App,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>>;
}

/// 注册文件 Item Provider。同一具体 Provider 类型只注册一次。
pub fn register_item_provider<P: ItemProvider>(provider: P, cx: &mut App) {
    ProviderRegistry::<dyn ItemProvider>::register(Arc::new(provider), TypeId::of::<P>(), cx);
}

/// 非文件标签的持久化恢复工厂。
///
/// 特殊 Item 只保存稳定类型与自身状态；
/// 恢复时由该工厂基于当前 Project 状态创建新视图。
pub trait SerializedItemProvider: Send + Sync + 'static {
    fn kind(&self) -> &'static str;

    fn restore(
        &self,
        state: serde_json::Value,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>>;
}

/// 注册非文件标签的持久化恢复工厂。同一具体 Provider 类型只注册一次。
pub fn register_serialized_item_provider<P: SerializedItemProvider>(provider: P, cx: &mut App) {
    ProviderRegistry::<dyn SerializedItemProvider>::register(
        Arc::new(provider),
        TypeId::of::<P>(),
        cx,
    );
}

/// 返回最后注册且支持该路径的 Item Provider。
pub(crate) fn item_provider_for_path(path: &Path, cx: &App) -> Option<Arc<dyn ItemProvider>> {
    ProviderRegistry::<dyn ItemProvider>::find(cx, |provider| provider.supports(path, cx))
}

/// 返回指定持久化类型的恢复工厂。
pub(crate) fn serialized_item_provider_for_kind(
    kind: &str,
    cx: &App,
) -> Option<Arc<dyn SerializedItemProvider>> {
    ProviderRegistry::<dyn SerializedItemProvider>::find(cx, |provider| provider.kind() == kind)
}

#[cfg(test)]
#[path = "test/item_provider_tests.rs"]
mod tests;
