//! ItemProvider 注册表：文件路径 → Workspace Item 的工厂。
//!
//! 具体文件类型（如文本编辑器）的 Item 构造由各 crate 注册，框架打开文件时经注册表分发，不依赖具体视图类型。

use std::any::TypeId;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, Entity, Task};
use zcv_project::Project;

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

/// 返回最后注册且支持该路径的 Item Provider。
pub(crate) fn item_provider_for_path(path: &Path, cx: &App) -> Option<Arc<dyn ItemProvider>> {
    ProviderRegistry::<dyn ItemProvider>::find(cx, |provider| provider.supports(path, cx))
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
            assert_eq!(
                cx.global::<ProviderRegistry<dyn ItemProvider>>()
                    .providers
                    .len(),
                1
            );
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
            let registry = cx.global::<ProviderRegistry<dyn ItemProvider>>();
            assert!(Arc::ptr_eq(
                &selected,
                &registry.providers.last().unwrap().provider
            ));
        });
    }
}
