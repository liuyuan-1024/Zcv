//! 文件预览的公共协议与注册表。
//!
//! 该 crate 不包含具体格式实现。
//! 格式 crate 实现 [`PreviewProvider`]，并创建一个直接实现 Workspace Item 协议的具体预览视图。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, BorrowAppContext, Entity, Global};
use zcv_editor::Editor;
use zcv_engine::Buffer;
use zcv_workspace::ItemHandle;

/// 稳定标识一种预览 Provider。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PreviewProviderId(pub &'static str);

/// Preview Provider 对宿主公开的元数据。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewDescriptor {
    pub id: PreviewProviderId,
    pub display_name: &'static str,
}

/// 交给 Preview Provider 的文档输入。
#[derive(Clone)]
pub struct PreviewDocument {
    pub path: PathBuf,
    pub source_editor: Entity<Editor>,
}

impl PreviewDocument {
    pub fn buffer(&self, cx: &App) -> Entity<Buffer> {
        self.source_editor.read(cx).buffer()
    }
}

/// 文件格式预览的工厂接口。
pub trait PreviewProvider: Send + Sync + 'static {
    fn descriptor(&self) -> PreviewDescriptor;
    fn supports(&self, path: &Path, cx: &App) -> bool;
    fn create(&self, document: PreviewDocument, cx: &mut App) -> Box<dyn ItemHandle>;
}

#[derive(Default)]
struct PreviewRegistry {
    providers: Vec<Arc<dyn PreviewProvider>>,
}

impl Global for PreviewRegistry {}

fn init_registry(cx: &mut App) {
    if !cx.has_global::<PreviewRegistry>() {
        cx.set_global(PreviewRegistry::default());
    }
}

/// 注册格式预览 Provider。同一 PreviewProviderId 只注册一次。
pub fn register(provider: impl PreviewProvider, cx: &mut App) {
    init_registry(cx);
    let id = provider.descriptor().id;
    cx.update_global::<PreviewRegistry, _>(|registry, _| {
        if registry
            .providers
            .iter()
            .all(|existing| existing.descriptor().id != id)
        {
            registry.providers.push(Arc::new(provider));
        }
    });
}

/// 返回第一个支持该路径的 Preview Provider。
pub fn provider_for(path: &Path, cx: &App) -> Option<Arc<dyn PreviewProvider>> {
    cx.try_global::<PreviewRegistry>()?
        .providers
        .iter()
        .find(|provider| provider.supports(path, cx))
        .cloned()
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    struct DiagramProvider;

    impl PreviewProvider for DiagramProvider {
        fn descriptor(&self) -> PreviewDescriptor {
            PreviewDescriptor {
                id: PreviewProviderId("diagram"),
                display_name: "Diagram",
            }
        }

        fn supports(&self, path: &Path, _cx: &App) -> bool {
            path.extension()
                .is_some_and(|extension| extension == "diagram")
        }

        fn create(&self, _document: PreviewDocument, _cx: &mut App) -> Box<dyn ItemHandle> {
            panic!("注册表匹配测试不应创建视图")
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
            assert_eq!(provider.descriptor().id, PreviewProviderId("diagram"));
            assert_eq!(provider.descriptor().display_name, "Diagram");
            assert!(provider_for(Path::new("architecture.txt"), cx).is_none());
            assert_eq!(cx.global::<PreviewRegistry>().providers.len(), 1);
        });
    }
}
