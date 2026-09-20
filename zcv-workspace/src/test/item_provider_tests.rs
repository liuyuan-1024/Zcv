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
