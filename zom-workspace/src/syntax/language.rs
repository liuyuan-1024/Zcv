//! Language registry：把 buffer 识别为语言 + 取对应 provider 工厂。
//!
//! 设计来自《桌面端语法高亮》§六。识别优先级：
//! 用户显式声明 > shebang > mode-line > 扩展名 > 默认 plain。
//! 当前调用方暂不提供「用户显式声明」通道，先按 shebang → mode-line → 扩展名
//! → filename → plain 顺序兜底。

use std::collections::BTreeMap;
use std::path::Path;

use super::provider::HighlightProvider;

/// 语言标识。
///
/// 与具体 grammar / provider 实现解耦——`LanguageId("rust")` 既可以走 native tree-sitter，也可以走 wasm 语言包，registry 端透明。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct LanguageId(&'static str);

impl LanguageId {
    pub const PLAIN: Self = Self("plain");

    pub const fn new(id: &'static str) -> Self {
        Self(id)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }

    pub fn is_plain(self) -> bool {
        self.0 == "plain"
    }
}

impl std::fmt::Display for LanguageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// 一条语言识别规则。`extension` 不含点（`"rs"` 而非 `".rs"`）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LanguageDetector {
    /// 扩展名匹配，全小写比较。
    Extension(&'static [&'static str]),
    /// 文件首行 shebang 命中的解释器名（如 `"rust-script"`）。
    Shebang(&'static [&'static str]),
    /// emacs/vim 风格 mode-line 中的 mode 名（如 `"ruby"`）。当前识别路径未消费；
    /// 保留枚举位，避免以后新增时改动公共枚举形状。
    ModeLine(&'static [&'static str]),
    /// 完整文件名匹配（如 `"Makefile"`）。
    Filename(&'static [&'static str]),
}

/// provider 工厂闭包别名。
pub type ProviderFactory = Box<dyn Fn() -> Box<dyn HighlightProvider> + Send + Sync>;

struct RegistryEntry {
    detectors: Vec<LanguageDetector>,
    factory: ProviderFactory,
}

impl std::fmt::Debug for RegistryEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryEntry")
            .field("detectors", &self.detectors)
            .finish_non_exhaustive()
    }
}

/// 语言注册表。
///
/// 一个 LanguageId 对应**一个** provider 工厂；后续接 LSP / wasm 时同样通过
/// `register` 注入，对调度层透明（手册 §十）。
#[derive(Debug, Default)]
pub struct LanguageRegistry {
    entries: BTreeMap<LanguageId, RegistryEntry>,
}

impl LanguageRegistry {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// 注册一门语言。同 id 二次注册会覆盖既有工厂，方便测试与启动期重装。
    pub fn register(
        &mut self,
        id: LanguageId,
        detectors: Vec<LanguageDetector>,
        factory: ProviderFactory,
    ) {
        self.entries
            .insert(id, RegistryEntry { detectors, factory });
    }

    /// 已注册的语言数量。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 实例化某语言的 provider；未注册返回 None。
    pub fn make_provider(&self, id: LanguageId) -> Option<Box<dyn HighlightProvider>> {
        self.entries.get(&id).map(|entry| (entry.factory)())
    }

    /// 按 path + 文件首行做识别。
    ///
    /// 顺序：shebang（首行命中且以 `#!` 开头）→ filename → extension →
    /// 默认 [`LanguageId::PLAIN`]。当前不消费 mode-line。
    ///
    /// `first_line` 拆成独立参数是为了让本方法保持**纯函数**——不依赖
    /// `Buffer` 类型、不做任何 IO，只看入参。带来两个收益：
    /// 1. 单测可以表驱动（见本文件 `tests` 模块），不需要起 buffer / 文件桩；
    /// 2. 拿不到首行的调用方（如「项目树预览：根据路径推测语言图标」
    ///    场景，根本没装 buffer）可以传 `None`，自然跳过 shebang 这一档。
    ///
    /// 现有调用方（[`crate::WorkspaceBuffer::attach_syntax`]）一律传首行——
    /// buffer 已在内存，取首行是 rope 切片，不是 IO。
    pub fn detect(&self, path: Option<&Path>, first_line: Option<&str>) -> LanguageId {
        if let Some(line) = first_line
            && let Some(rest) = line.strip_prefix("#!")
            && let Some(id) = self.detect_by_shebang(rest)
        {
            return id;
        }

        if let Some(path) = path {
            if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && let Some(id) = self.detect_by_filename(name)
            {
                return id;
            }

            if let Some(ext) = path.extension().and_then(|e| e.to_str())
                && let Some(id) = self.detect_by_extension(ext)
            {
                return id;
            }
        }

        LanguageId::PLAIN
    }

    fn detect_by_extension(&self, ext: &str) -> Option<LanguageId> {
        let lower = ext.to_ascii_lowercase();
        for (id, entry) in &self.entries {
            for detector in &entry.detectors {
                if let LanguageDetector::Extension(exts) = detector
                    && exts.iter().any(|e| e.eq_ignore_ascii_case(&lower))
                {
                    return Some(*id);
                }
            }
        }
        None
    }

    fn detect_by_filename(&self, name: &str) -> Option<LanguageId> {
        for (id, entry) in &self.entries {
            for detector in &entry.detectors {
                if let LanguageDetector::Filename(names) = detector
                    && names.iter().any(|n| *n == name)
                {
                    return Some(*id);
                }
            }
        }
        None
    }

    /// `rest` 是 `#!` 之后的剩余字符串。命中规则：rest 经 ASCII 空白分隔后的
    /// 任一 token 与注册的 shebang 名相等即可——这样既覆盖 `#!/usr/bin/rust`
    /// 也覆盖 `#!/usr/bin/env rust-script`。
    fn detect_by_shebang(&self, rest: &str) -> Option<LanguageId> {
        for (id, entry) in &self.entries {
            for detector in &entry.detectors {
                if let LanguageDetector::Shebang(names) = detector {
                    for token in rest.split(|c: char| c.is_ascii_whitespace() || c == '/') {
                        if token.is_empty() {
                            continue;
                        }
                        if names.iter().any(|n| *n == token) {
                            return Some(*id);
                        }
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::BufferHandle;
    use crate::syntax::provider::HighlightProvider;
    use zom_engine::{BufferVersion, ChangeSet};

    #[derive(Debug)]
    struct MockProvider {
        id: LanguageId,
    }

    impl HighlightProvider for MockProvider {
        fn language(&self) -> LanguageId {
            self.id
        }
        fn attach(&mut self, _buffer: BufferHandle) {}
        fn on_edit(&mut self, _buffer: BufferHandle, _change: &ChangeSet, _version: BufferVersion) {
        }
        fn detach(&mut self) {}
    }

    fn make_registry() -> LanguageRegistry {
        let mut reg = LanguageRegistry::new();
        reg.register(
            LanguageId::new("rust"),
            vec![
                LanguageDetector::Extension(&["rs"]),
                LanguageDetector::Shebang(&["rust-script"]),
            ],
            Box::new(|| {
                Box::new(MockProvider {
                    id: LanguageId::new("rust"),
                })
            }),
        );
        reg.register(
            LanguageId::new("make"),
            vec![LanguageDetector::Filename(&["Makefile", "GNUmakefile"])],
            Box::new(|| {
                Box::new(MockProvider {
                    id: LanguageId::new("make"),
                })
            }),
        );
        reg
    }

    #[test]
    fn detect_by_extension() {
        let reg = make_registry();
        assert_eq!(
            reg.detect(Some(Path::new("foo/bar.rs")), None),
            LanguageId::new("rust")
        );
        // 大小写不敏感
        assert_eq!(
            reg.detect(Some(Path::new("FOO.RS")), None),
            LanguageId::new("rust")
        );
    }

    #[test]
    fn detect_plain_for_unknown_extension() {
        let reg = make_registry();
        assert_eq!(
            reg.detect(Some(Path::new("notes.txt")), None),
            LanguageId::PLAIN
        );
    }

    #[test]
    fn detect_by_filename_takes_priority_over_extension() {
        let reg = make_registry();
        assert_eq!(
            reg.detect(Some(Path::new("project/Makefile")), None),
            LanguageId::new("make")
        );
    }

    #[test]
    fn detect_by_shebang() {
        let reg = make_registry();
        assert_eq!(
            reg.detect(None, Some("#!/usr/bin/env rust-script")),
            LanguageId::new("rust")
        );
        assert_eq!(
            reg.detect(
                Some(Path::new("script.unknown")),
                Some("#!/usr/bin/rust-script")
            ),
            LanguageId::new("rust")
        );
    }

    #[test]
    fn detect_returns_plain_when_no_match() {
        let reg = make_registry();
        assert_eq!(reg.detect(None, None), LanguageId::PLAIN);
        assert_eq!(
            reg.detect(None, Some("// not a shebang")),
            LanguageId::PLAIN
        );
    }

    #[test]
    fn make_provider_returns_none_for_unregistered() {
        let reg = make_registry();
        assert!(reg.make_provider(LanguageId::new("python")).is_none());
        assert!(reg.make_provider(LanguageId::new("rust")).is_some());
    }
}
