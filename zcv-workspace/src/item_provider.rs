//! ItemProvider 注册表：文件路径 → Workspace Item 的工厂。
//!
//! 对齐 Zed 的 WorkspaceItemBuilder 机制：具体文件类型（如文本编辑器）的Item 构造由各 crate 注册，框架打开文件时经注册表分发，不依赖具体视图类型。

use std::any::TypeId;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, BorrowAppContext, Entity, Global, Task};
use zcv_project::Project;

use crate::item::ItemHandle;

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

struct RegisteredItemProvider {
    type_id: TypeId,
    provider: Arc<dyn ItemProvider>,
}

#[derive(Default)]
struct ItemProviderRegistry {
    providers: Vec<RegisteredItemProvider>,
}

impl Global for ItemProviderRegistry {}

/// 注册文件 Item Provider。同一具体 Provider 类型只注册一次。
pub fn register_item_provider<P: ItemProvider>(provider: P, cx: &mut App) {
    if !cx.has_global::<ItemProviderRegistry>() {
        cx.set_global(ItemProviderRegistry::default());
    }
    let type_id = TypeId::of::<P>();
    cx.update_global::<ItemProviderRegistry, _>(|registry, _| {
        if registry
            .providers
            .iter()
            .all(|entry| entry.type_id != type_id)
        {
            registry.providers.push(RegisteredItemProvider {
                type_id,
                provider: Arc::new(provider),
            });
        }
    });
}

/// 返回最后注册且支持该路径的 Item Provider。
pub fn item_provider_for_path(path: &Path, cx: &App) -> Option<Arc<dyn ItemProvider>> {
    cx.try_global::<ItemProviderRegistry>()?
        .providers
        .iter()
        .rev()
        .find(|entry| entry.provider.supports(path, cx))
        .map(|entry| Arc::clone(&entry.provider))
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    struct TextProvider;

    struct LaterTextProvider;

    impl ItemProvider for TextProvider {
        fn supports(&self, path: &Path, _cx: &App) -> bool {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension == "txt")
        }

        fn open_item(
            &self,
            _path: PathBuf,
            _project: Entity<Project>,
            _cx: &mut App,
        ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
            panic!("注册表匹配测试不应创建 Item")
        }
    }

    impl ItemProvider for LaterTextProvider {
        fn supports(&self, path: &Path, _cx: &App) -> bool {
            path.extension().is_some_and(|extension| extension == "txt")
        }

        fn open_item(
            &self,
            _path: PathBuf,
            _project: Entity<Project>,
            _cx: &mut App,
        ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
            panic!("注册表优先级测试不应创建 Item")
        }
    }

    #[gpui::test]
    fn provider_is_discovered_and_duplicate_registration_is_ignored(cx: &mut TestAppContext) {
        cx.update(|cx| {
            register_item_provider(TextProvider, cx);
            register_item_provider(TextProvider, cx);
        });

        cx.read(|cx| {
            let provider = item_provider_for_path(Path::new("demo.txt"), cx)
                .expect("txt 应由注册的 Provider 匹配");
            assert!(provider.supports(Path::new("demo.txt"), cx));
            assert!(item_provider_for_path(Path::new("demo.rs"), cx).is_none());
            assert_eq!(cx.global::<ItemProviderRegistry>().providers.len(), 1);
        });
    }

    #[gpui::test]
    fn last_registered_matching_provider_takes_priority(cx: &mut TestAppContext) {
        cx.update(|cx| {
            register_item_provider(TextProvider, cx);
            register_item_provider(LaterTextProvider, cx);
        });

        cx.read(|cx| {
            let selected = item_provider_for_path(Path::new("demo.txt"), cx).unwrap();
            let registry = cx.global::<ItemProviderRegistry>();
            assert!(Arc::ptr_eq(
                &selected,
                &registry.providers.last().unwrap().provider
            ));
        });
    }
}
