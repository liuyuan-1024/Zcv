//! 语言注册与查询。

use std::path::Path;
use std::sync::{Arc, OnceLock};

use tree_sitter::Query;

use crate::available_languages::{QuerySource, builtin_languages, language_queries};

/// 一门可由 tree-sitter 解析和高亮的语言。
#[derive(Clone, Debug)]
pub struct Language {
    name: &'static str,
    grammar: tree_sitter::Language,
    highlights: Arc<Query>,
    injections: Option<Arc<Query>>,
    queries: LanguageQueries,
    capture_names: Arc<[Arc<str>]>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LanguageQueries {
    pub(crate) brackets: Option<Arc<Query>>,
    pub(crate) indents: Option<Arc<Query>>,
    pub(crate) outline: Option<Arc<Query>>,
    pub(crate) text_objects: Option<Arc<Query>>,
    pub(crate) folds: Option<Arc<Query>>,
}

impl Language {
    pub fn name(&self) -> &'static str {
        self.name
    }

    pub(crate) fn grammar(&self) -> &tree_sitter::Language {
        &self.grammar
    }

    pub(crate) fn highlights(&self) -> &Arc<Query> {
        &self.highlights
    }

    pub(crate) fn injections(&self) -> Option<&Arc<Query>> {
        self.injections.as_ref()
    }

    pub(crate) fn brackets(&self) -> Option<&Arc<Query>> {
        self.queries.brackets.as_ref()
    }

    pub(crate) fn indents(&self) -> Option<&Arc<Query>> {
        self.queries.indents.as_ref()
    }

    pub(crate) fn outline(&self) -> Option<&Arc<Query>> {
        self.queries.outline.as_ref()
    }

    pub(crate) fn text_objects(&self) -> Option<&Arc<Query>> {
        self.queries.text_objects.as_ref()
    }

    pub(crate) fn folds(&self) -> Option<&Arc<Query>> {
        self.queries.folds.as_ref()
    }

    /// capture 名字表（capture index -> 名字），供跨语言全局表构建与渲染查表使用。
    pub(crate) fn capture_names(&self) -> &[Arc<str>] {
        &self.capture_names
    }
}

/// 文件识别规则（对齐 Zed `LanguageMatcher`）：扩展名 + 首行模式。
///
/// 首行模式在注册表构建时一次性编译（内置模式静态正确，编译失败直接 panic），
/// 文件匹配热路径不再每次重新编译正则。
pub(crate) struct LanguageMatcher {
    pub(crate) suffixes: &'static [&'static str],
    pub(crate) first_line_pattern: Option<regex::Regex>,
}

/// 语言注册条目（对齐 Zed `AvailableLanguage`）：声明 + 惰性加载。
///
/// 声明（名称/matcher）在注册表构建时登记，文件识别不依赖语言已加载；
/// `load` 在真正需要语法解析/高亮时才编译 grammar 与查询。
pub(crate) struct LanguageEntry {
    pub(crate) name: &'static str,
    pub(crate) matcher: LanguageMatcher,
    pub(crate) grammar: Option<fn() -> tree_sitter::Language>,
    pub(crate) highlights: Option<QuerySource>,
    pub(crate) injections: Option<QuerySource>,
    /// 注入查询使用的别名（如 markdown 的 `markdown_inline`）；仅注入查找用，不参与文件识别。
    pub(crate) injection_alias: Option<&'static str>,
}

impl LanguageEntry {
    fn load(&self) -> Option<Language> {
        let grammar = (self.grammar?)();
        let highlights = self.highlights?.compile(&grammar).ok()?;
        let injections = self
            .injections
            .map(|source| source.compile(&grammar))
            .transpose()
            .ok()?;
        let queries = language_queries(self.name, &grammar)?;
        let capture_names = highlights
            .capture_names()
            .iter()
            .map(|name| Arc::<str>::from(*name))
            .collect();
        Some(Language {
            name: self.name,
            grammar,
            highlights: Arc::new(highlights),
            injections: injections.map(Arc::new),
            queries,
            capture_names,
        })
    }

    /// 注入名匹配：别名（如 `markdown_inline`）、语言名与后缀均忽略大小写。
    fn matches_injection_name(&self, name: &str) -> bool {
        let name = name.trim();
        self.injection_alias
            .is_some_and(|alias| alias.eq_ignore_ascii_case(name))
            || self.name.eq_ignore_ascii_case(name)
            || self
                .matcher
                .suffixes
                .iter()
                .any(|suffix| suffix.eq_ignore_ascii_case(name))
    }
}

/// 语言注册表（对齐 Zed `LanguageRegistry`）：语言声明 + 文件识别 + 惰性加载。
///
/// 当前尚无扩展系统，注册条目来自内置语言数据（`builtin_languages`）；
/// 若未来支持用户配置覆盖 matcher，在此注册表上扩展即可。
pub(crate) struct LanguageRegistry {
    languages: Vec<LanguageEntry>,
}

pub(crate) fn registry() -> &'static LanguageRegistry {
    static REGISTRY: OnceLock<LanguageRegistry> = OnceLock::new();
    REGISTRY.get_or_init(LanguageRegistry::new)
}

impl LanguageRegistry {
    fn new() -> Self {
        Self {
            languages: builtin_languages(),
        }
    }

    /// 按注入名查语言（语法树注入层使用）。
    pub(crate) fn language_for_injection(&self, name: &str) -> Option<Language> {
        self.languages
            .iter()
            .find(|entry| entry.matches_injection_name(name))
            .and_then(LanguageEntry::load)
    }

    /// 按文件名和首行内容选择已注册且可高亮的语言。
    pub(crate) fn language_for_file(
        &self,
        path: &Path,
        first_line: Option<&str>,
    ) -> Option<Language> {
        self.matched_language(path, first_line)
            .and_then(|entry| entry.load())
    }

    /// 返回语言显示名；即使暂未注册 grammar，也保留文件类型识别能力。
    pub(crate) fn language_name_for_file(
        &self,
        path: &Path,
        first_line: Option<&str>,
    ) -> Option<&'static str> {
        self.matched_language(path, first_line)
            .map(|entry| entry.name)
    }

    fn matched_language(&self, path: &Path, first_line: Option<&str>) -> Option<&LanguageEntry> {
        let filename = path.file_name()?.to_str()?;
        let mut matched = self
            .languages
            .iter()
            .flat_map(|entry| {
                entry
                    .matcher
                    .suffixes
                    .iter()
                    .filter(move |suffix| ends_with_dot_suffix(filename, suffix))
                    .map(move |_| entry)
            })
            .max_by_key(|entry| entry.matcher.suffixes.iter().map(|s| s.len()).max());

        if matched.is_none()
            && let Some(first_line) = first_line
        {
            matched = self.languages.iter().find_map(|entry| {
                entry
                    .matcher
                    .first_line_pattern
                    .as_ref()?
                    .is_match(first_line)
                    .then_some(entry)
            });
        }

        matched
    }
}

/// 文件名以 `.suffix` 结尾（零分配判断，避免 `format!` 拼接临时字符串）。
fn ends_with_dot_suffix(filename: &str, suffix: &str) -> bool {
    filename
        .strip_suffix(suffix)
        .is_some_and(|stem| stem.ends_with('.'))
}

// ── 顶层查询入口（调用方经此处访问注册表）────────────────────────────

/// 按注入名查语言（语法树注入层使用）。
pub(crate) fn language_for_injection(name: &str) -> Option<Language> {
    registry().language_for_injection(name)
}

/// 根据文件名和首行内容选择已注册且可高亮的语言。
pub(crate) fn language_for_file(path: &Path, first_line: Option<&str>) -> Option<Language> {
    registry().language_for_file(path, first_line)
}

/// 返回语言显示名；即使暂未注册 grammar，也保留文件类型识别能力。
pub(crate) fn language_name_for_file(
    path: &Path,
    first_line: Option<&str>,
) -> Option<&'static str> {
    registry().language_name_for_file(path, first_line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_rust_and_tsx_with_distinct_grammars() {
        assert_eq!(
            language_for_file(Path::new("main.rs"), None)
                .unwrap()
                .name(),
            "Rust"
        );
        assert_eq!(
            language_for_file(Path::new("view.tsx"), None)
                .unwrap()
                .name(),
            "TSX"
        );
    }

    #[test]
    fn detects_shell_from_shebang() {
        assert_eq!(
            language_for_file(Path::new("script"), Some("#!/usr/bin/env bash"))
                .unwrap()
                .name(),
            "Shell"
        );
    }

    #[test]
    fn javascript_family_compiles_declared_query_layers() {
        let jsx = language_for_file(Path::new("view.jsx"), None).unwrap();
        assert!(jsx.highlights.capture_names().contains(&"variable"));
        assert!(jsx.highlights.capture_names().contains(&"tag"));
        assert!(jsx.injections.is_some());

        let typescript = language_for_file(Path::new("main.ts"), None).unwrap();
        assert!(typescript.highlights.capture_names().contains(&"variable"));
        assert!(typescript.highlights.capture_names().contains(&"type"));
        assert!(typescript.injections.is_some());

        let tsx = language_for_file(Path::new("view.tsx"), None).unwrap();
        assert!(tsx.highlights.capture_names().contains(&"variable"));
        assert!(tsx.highlights.capture_names().contains(&"tag"));
        assert!(tsx.highlights.capture_names().contains(&"type"));
        assert!(tsx.injections.is_some());
    }

    #[test]
    fn zed_structure_queries_compile_for_supported_grammars() {
        for path in [
            "main.rs",
            "main.py",
            "main.js",
            "view.ts",
            "view.tsx",
            "script.sh",
            "README.md",
            "index.html",
            "style.css",
            "data.json",
            "data.yaml",
        ] {
            let language = language_for_file(Path::new(path), None)
                .unwrap_or_else(|| panic!("{path} 的 Zed 结构查询应与 grammar 匹配"));
            assert!(language.brackets().is_some(), "{path} 应提供括号查询");
            assert!(
                language.indents().is_some() || path.ends_with(".yaml"),
                "{path} 应提供缩进查询"
            );
            assert!(
                language.outline().is_some() || path.ends_with(".sh"),
                "{path} 应提供大纲查询"
            );
            assert!(
                language.text_objects().is_some() || path.ends_with(".html"),
                "{path} 应提供文本对象查询"
            );
            assert!(
                language.folds().is_some() || !path.ends_with(".rs"),
                "{path} 应提供折叠查询"
            );
        }
    }
}
