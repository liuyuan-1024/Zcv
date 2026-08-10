//! ItemProvider 注册表：文件路径 → Workspace Item 的工厂。
//!
//! 对齐 Zed 的 WorkspaceItemBuilder 机制：具体文件类型（如文本编辑器）的Item 构造由各 crate 注册，框架打开文件时经注册表分发，不依赖具体视图类型。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, BorrowAppContext, Entity, Global, Task};
use zcv_project::Project;

use crate::item::ItemHandle;
use crate::preview::PreviewProviderId;

/// ItemProvider 对宿主公开的元数据。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemProviderDescriptor {
    pub id: PreviewProviderId,
    pub display_name: &'static str,
}

/// 文件路径 → Workspace Item 的工厂接口。
pub trait ItemProvider: Send + Sync + 'static {
    fn descriptor(&self) -> ItemProviderDescriptor;

    fn supports(&self, path: &Path, cx: &App) -> bool;

    /// 为路径构造 Item；项目 buffer 打开等异步工作由 provider 自行完成。
    fn open_item(
        &self,
        path: PathBuf,
        project: Entity<Project>,
        cx: &mut App,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>>;
}

#[derive(Default)]
struct ItemProviderRegistry {
    providers: Vec<Arc<dyn ItemProvider>>,
}

impl Global for ItemProviderRegistry {}

/// 注册文件 Item Provider。同一 id 只注册一次。
pub fn register_item_provider(provider: impl ItemProvider, cx: &mut App) {
    if !cx.has_global::<ItemProviderRegistry>() {
        cx.set_global(ItemProviderRegistry::default());
    }
    let id = provider.descriptor().id;
    cx.update_global::<ItemProviderRegistry, _>(|registry, _| {
        if registry
            .providers
            .iter()
            .all(|existing| existing.descriptor().id != id)
        {
            registry.providers.push(Arc::new(provider));
        }
    });
}

/// 返回第一个支持该路径的 Item Provider。
pub fn item_provider_for_path(path: &Path, cx: &App) -> Option<Arc<dyn ItemProvider>> {
    cx.try_global::<ItemProviderRegistry>()?
        .providers
        .iter()
        .find(|provider| provider.supports(path, cx))
        .cloned()
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    struct TextProvider;

    impl ItemProvider for TextProvider {
        fn descriptor(&self) -> ItemProviderDescriptor {
            ItemProviderDescriptor {
                id: PreviewProviderId("text"),
                display_name: "Text",
            }
        }

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

    #[gpui::test]
    fn provider_is_discovered_and_duplicate_registration_is_ignored(cx: &mut TestAppContext) {
        cx.update(|cx| {
            register_item_provider(TextProvider, cx);
            register_item_provider(TextProvider, cx);
        });

        cx.read(|cx| {
            let provider = item_provider_for_path(Path::new("demo.txt"), cx)
                .expect("txt 应由注册的 Provider 匹配");
            assert_eq!(provider.descriptor().id, PreviewProviderId("text"));
            assert!(item_provider_for_path(Path::new("demo.rs"), cx).is_none());
            assert_eq!(cx.global::<ItemProviderRegistry>().providers.len(), 1);
        });
    }
}
