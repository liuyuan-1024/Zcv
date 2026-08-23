//! 文件预览的公共协议与注册表。
//!
//! 该模块不包含具体格式实现。
//! 格式 crate 实现 [`PreviewProvider`]，并创建一个直接实现 Item 协议的具体预览视图。
//! 预览视图自身通过 [`PreviewItem`] 暴露与源码 Item 的关联，经 `Item::as_preview_item` 桥接获取（对齐 Zed 的 `as_searchable` 模式），不占用 Item 主接口。

use std::any::TypeId;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, Entity};

use crate::item::{Item, ItemHandle};
use crate::provider_registry::ProviderRegistry;

/// 交给 Preview Provider 的文档输入：预览视图的源码 Item 与展示路径。
#[derive(Clone)]
pub struct PreviewDocument {
    pub path: PathBuf,
    pub source_item: Box<dyn ItemHandle>,
}

/// 预览视图 Item 的 object-safe 句柄，经 `Item::as_preview_item` 获取。
pub trait PreviewItemHandle: Send + 'static {
    /// 预览视图对应的源码 Item（通常是编辑器）；无法暴露时返回 None。
    fn source_item(&self, cx: &App) -> Option<Box<dyn ItemHandle>>;
}

/// 预览视图 Item 的协议：提供与源码 Item 的关联。
pub trait PreviewItem: Item {
    fn source_item(&self, _cx: &App) -> Option<Box<dyn ItemHandle>> {
        None
    }
}

impl<T: PreviewItem> PreviewItemHandle for Entity<T> {
    fn source_item(&self, cx: &App) -> Option<Box<dyn ItemHandle>> {
        self.read(cx).source_item(cx)
    }
}

/// 文件格式预览的工厂接口。
pub trait PreviewProvider: Send + Sync + 'static {
    fn supports(&self, path: &Path, cx: &App) -> bool;

    fn create(&self, document: PreviewDocument, cx: &mut App) -> Box<dyn ItemHandle>;
}

/// 注册格式预览 Provider。同一具体 Provider 类型只注册一次。
pub fn register<P: PreviewProvider>(provider: P, cx: &mut App) {
    ProviderRegistry::<dyn PreviewProvider>::register(Arc::new(provider), TypeId::of::<P>(), cx);
}

/// 返回最后注册且支持该路径的 Preview Provider。
pub fn provider_for(path: &Path, cx: &App) -> Option<Arc<dyn PreviewProvider>> {
    ProviderRegistry::<dyn PreviewProvider>::find(cx, |provider| provider.supports(path, cx))
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    struct DiagramProvider;

    struct LaterDiagramProvider;

    impl PreviewProvider for DiagramProvider {
        fn supports(&self, path: &Path, _cx: &App) -> bool {
            path.extension()
                .is_some_and(|extension| extension == "diagram")
        }

        fn create(&self, _document: PreviewDocument, _cx: &mut App) -> Box<dyn ItemHandle> {
            panic!("注册表匹配测试不应创建视图")
        }
    }

    impl PreviewProvider for LaterDiagramProvider {
        fn supports(&self, path: &Path, _cx: &App) -> bool {
            path.extension()
                .is_some_and(|extension| extension == "diagram")
        }

        fn create(&self, _document: PreviewDocument, _cx: &mut App) -> Box<dyn ItemHandle> {
            panic!("注册表优先级测试不应创建视图")
        }
    }

    #[gpui::test]
    fn provider_is_discovered_and_duplicate_registration_is_ignored(cx: &mut TestAppContext) {
        cx.update(|cx| {
            register(DiagramProvider, cx);
            register(DiagramProvider, cx);
        });

        cx.read(|cx| {
            let provider = provider_for(Path::new("architecture.diagram"), cx)
                .expect("新格式应由注册的 Provider 匹配");
            assert!(provider.supports(Path::new("architecture.diagram"), cx));
            assert!(provider_for(Path::new("architecture.txt"), cx).is_none());
            assert_eq!(
                cx.global::<ProviderRegistry<dyn PreviewProvider>>()
                    .providers
                    .len(),
                1
            );
        });
    }

    #[gpui::test]
    fn last_registered_matching_provider_takes_priority(cx: &mut TestAppContext) {
        cx.update(|cx| {
            register(DiagramProvider, cx);
            register(LaterDiagramProvider, cx);
        });

        cx.read(|cx| {
            let selected = provider_for(Path::new("architecture.diagram"), cx).unwrap();
            let registry = cx.global::<ProviderRegistry<dyn PreviewProvider>>();
            assert!(Arc::ptr_eq(
                &selected,
                &registry.providers.last().unwrap().provider
            ));
        });
    }
}
