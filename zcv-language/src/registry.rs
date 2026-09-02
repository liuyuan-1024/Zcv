//! 语言注册与查询。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use tree_sitter::Query;

use crate::AutoClosePair;
use crate::available_languages::{
    LanguageQuerySources, LanguageSpec, LanguageSupport as LanguageSupportSpec, builtin_languages,
};

/// 一门已加载语言。
#[derive(Debug)]
pub struct Language {
    name: &'static str,
    syntax: LanguageSyntax,
    auto_close_pairs: &'static [AutoClosePair],
}

#[derive(Debug)]
enum LanguageSyntax {
    PlainText,
    TreeSitter {
        grammar: tree_sitter::Language,
        queries: CompiledLanguageQueries,
        capture_names: Arc<[Arc<str>]>,
    },
}

#[derive(Debug)]
struct CompiledLanguageQueries {
    highlights: Arc<Query>,
    injections: Option<Arc<Query>>,
    brackets: Option<Arc<Query>>,
    indents: Option<Arc<Query>>,
    folds: Option<Arc<Query>>,
}

impl Language {
    pub fn name(&self) -> &'static str {
        self.name
    }

    pub(crate) fn grammar(&self) -> Option<&tree_sitter::Language> {
        match &self.syntax {
            LanguageSyntax::PlainText => None,
            LanguageSyntax::TreeSitter { grammar, .. } => Some(grammar),
        }
    }

    pub(crate) fn highlights(&self) -> Option<&Arc<Query>> {
        match &self.syntax {
            LanguageSyntax::PlainText => None,
            LanguageSyntax::TreeSitter { queries, .. } => Some(&queries.highlights),
        }
    }

    pub(crate) fn injections(&self) -> Option<&Arc<Query>> {
        match &self.syntax {
            LanguageSyntax::PlainText => None,
            LanguageSyntax::TreeSitter { queries, .. } => queries.injections.as_ref(),
        }
    }

    pub(crate) fn brackets(&self) -> Option<&Arc<Query>> {
        match &self.syntax {
            LanguageSyntax::PlainText => None,
            LanguageSyntax::TreeSitter { queries, .. } => queries.brackets.as_ref(),
        }
    }

    pub(crate) fn indents(&self) -> Option<&Arc<Query>> {
        match &self.syntax {
            LanguageSyntax::PlainText => None,
            LanguageSyntax::TreeSitter { queries, .. } => queries.indents.as_ref(),
        }
    }

    pub(crate) fn folds(&self) -> Option<&Arc<Query>> {
        match &self.syntax {
            LanguageSyntax::PlainText => None,
            LanguageSyntax::TreeSitter { queries, .. } => queries.folds.as_ref(),
        }
    }

    /// 输入级自动闭合配对表（编辑器输入行为的数据源）。
    pub fn auto_close_pairs(&self) -> &'static [AutoClosePair] {
        self.auto_close_pairs
    }

    /// capture 名字表（capture index -> 名字），供跨语言全局表构建与渲染查表使用。
    pub(crate) fn capture_names(&self) -> &[Arc<str>] {
        match &self.syntax {
            LanguageSyntax::PlainText => &[],
            LanguageSyntax::TreeSitter { capture_names, .. } => capture_names,
        }
    }
}

impl LanguageSpec {
    fn load(&self) -> Language {
        let syntax = match &self.support {
            LanguageSupportSpec::PlainText => LanguageSyntax::PlainText,
            LanguageSupportSpec::TreeSitter { grammar, queries } => {
                let grammar = grammar();
                let queries = compile_queries(self.name, &grammar, *queries);
                let capture_names = queries
                    .highlights
                    .capture_names()
                    .iter()
                    .map(|name| Arc::<str>::from(*name))
                    .collect();
                LanguageSyntax::TreeSitter {
                    grammar,
                    queries,
                    capture_names,
                }
            }
        };
        Language {
            name: self.name,
            syntax,
            auto_close_pairs: self.auto_close_pairs,
        }
    }
}

fn compile_queries(
    language_name: &str,
    grammar: &tree_sitter::Language,
    sources: LanguageQuerySources,
) -> CompiledLanguageQueries {
    CompiledLanguageQueries {
        highlights: compile_query(language_name, "高亮", grammar, sources.highlights),
        injections: compile_optional_query(language_name, "注入", grammar, sources.injections),
        brackets: compile_optional_query(language_name, "括号", grammar, sources.brackets),
        indents: compile_optional_query(language_name, "缩进", grammar, sources.indents),
        folds: compile_optional_query(language_name, "折叠", grammar, sources.folds),
    }
}

fn compile_optional_query(
    language_name: &str,
    query_name: &str,
    grammar: &tree_sitter::Language,
    source: Option<&str>,
) -> Option<Arc<Query>> {
    source.map(|source| compile_query(language_name, query_name, grammar, source))
}

fn compile_query(
    language_name: &str,
    query_name: &str,
    grammar: &tree_sitter::Language,
    source: &str,
) -> Arc<Query> {
    Arc::new(
        Query::new(grammar, source)
            .unwrap_or_else(|error| panic!("{language_name} {query_name}查询编译失败：{error}")),
    )
}

/// 语言注册表：持有内置规格，负责文件识别与惰性加载。
pub(crate) struct LanguageRegistry {
    languages: Vec<LanguageSpec>,
    loaded_languages: Mutex<HashMap<&'static str, Arc<Language>>>,
}

pub(crate) fn registry() -> &'static LanguageRegistry {
    static REGISTRY: OnceLock<LanguageRegistry> = OnceLock::new();
    REGISTRY.get_or_init(LanguageRegistry::new)
}

impl LanguageRegistry {
    fn new() -> Self {
        Self {
            languages: builtin_languages(),
            loaded_languages: Mutex::new(HashMap::new()),
        }
    }

    fn load(&self, entry: &LanguageSpec) -> Arc<Language> {
        let mut loaded = self.loaded_languages.lock().expect("语言缓存锁不应中毒");
        Arc::clone(
            loaded
                .entry(entry.name)
                .or_insert_with(|| Arc::new(entry.load())),
        )
    }

    /// 按注入名查语言（语法树注入层使用）。
    pub(crate) fn language_for_injection(&self, name: &str) -> Option<Arc<Language>> {
        self.languages
            .iter()
            .find(|entry| entry.matches_injection_name(name))
            .map(|entry| self.load(entry))
    }

    /// 按文件名和首行内容选择语言规格。
    pub(crate) fn language_for_file(
        &self,
        path: &Path,
        first_line: Option<&str>,
    ) -> Option<Arc<Language>> {
        self.matched_language(path, first_line)
            .map(|entry| self.load(entry))
    }

    fn matched_language(&self, path: &Path, first_line: Option<&str>) -> Option<&LanguageSpec> {
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

        // 兜底：任何未识别文件都以纯文本打开，编辑器始终有语言名可显示。
        matched.or_else(|| {
            self.languages
                .iter()
                .find(|entry| matches!(&entry.support, LanguageSupportSpec::PlainText))
        })
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
pub(crate) fn language_for_injection(name: &str) -> Option<Arc<Language>> {
    registry().language_for_injection(name)
}

/// 根据文件名和首行内容选择语言规格。
/// 根据文件名和首行内容识别语言。
///
/// 文件类型消费方应通过此接口共享语言识别结果，不能各自维护后缀匹配表。
pub fn language_for_file(path: &Path, first_line: Option<&str>) -> Option<Arc<Language>> {
    registry().language_for_file(path, first_line)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

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
    fn detects_baseline_languages_by_suffix() {
        for (path, expected) in [
            ("main.c", "C"),
            ("main.cpp", "C++"),
            ("main.hpp", "C++"),
            ("Program.cs", "C#"),
            ("main.go", "Go"),
            ("app.rb", "Ruby"),
            ("index.php", "PHP"),
            ("main.swift", "Swift"),
            ("Main.kt", "Kotlin"),
            ("build.gradle.kts", "Kotlin"),
            ("init.lua", "Lua"),
            ("main.zig", "Zig"),
            ("query.sql", "SQL"),
        ] {
            assert_eq!(
                language_for_file(Path::new(path), None).unwrap().name(),
                expected,
                "{path} 应识别为 {expected}"
            );
        }
    }

    #[test]
    fn detects_baseline_script_languages_from_first_line() {
        for (first_line, expected) in [
            ("#!/usr/bin/env ruby", "Ruby"),
            ("#!/usr/bin/php", "PHP"),
            ("#!/usr/bin/env swift", "Swift"),
            ("#!/usr/bin/env lua5.4", "Lua"),
            ("//usr/bin/env go run $0 $@; exit", "Go"),
        ] {
            assert_eq!(
                language_for_file(Path::new("script"), Some(first_line))
                    .unwrap()
                    .name(),
                expected,
                "`{first_line}` 应识别为 {expected}"
            );
        }
    }

    #[test]
    fn reuses_loaded_language_and_compiled_queries() {
        let first = language_for_file(Path::new("main.rs"), None).unwrap();
        let second = language_for_file(Path::new("lib.rs"), None).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(Arc::ptr_eq(
            first.highlights().unwrap(),
            second.highlights().unwrap()
        ));
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
    fn unknown_files_fall_back_to_plain_text() {
        // 未支持的语言后缀也必须明确回落为纯文本，不能注册成缺少 grammar 的半支持语言。
        for path in ["main.dart", "main.ex", "main.hs", "query.graphql"] {
            let language = language_for_file(Path::new(path), None).unwrap();
            assert_eq!(language.name(), "纯文本", "{path} 尚未提供完整语言规格");
            assert!(language.grammar().is_none());
        }

        // .gitignore 等无扩展名文件与未知后缀都以纯文本兜底，语言名始终可显示。
        assert_eq!(
            language_for_file(Path::new(".gitignore"), None)
                .unwrap()
                .name(),
            "纯文本"
        );
        assert_eq!(
            language_for_file(Path::new("Makefile"), None)
                .unwrap()
                .name(),
            "纯文本"
        );
        assert_eq!(
            language_for_file(Path::new("archive.unknown_ext"), None)
                .unwrap()
                .name(),
            "纯文本"
        );
        // .txt 显式匹配 Plain Text；无语法树语言不产出高亮查询。
        let plain = language_for_file(Path::new("notes.txt"), None).unwrap();
        assert_eq!(plain.name(), "纯文本");
        assert!(plain.highlights().is_none(), "纯文本语言不应有高亮查询");
    }

    #[test]
    fn javascript_family_compiles_declared_query_layers() {
        let jsx = language_for_file(Path::new("view.jsx"), None).unwrap();
        assert!(
            jsx.highlights()
                .unwrap()
                .capture_names()
                .contains(&"variable")
        );
        assert!(
            jsx.highlights()
                .unwrap()
                .capture_names()
                .contains(&"tag.jsx")
        );
        assert!(jsx.injections().is_some());

        let typescript = language_for_file(Path::new("main.ts"), None).unwrap();
        assert!(
            typescript
                .highlights()
                .unwrap()
                .capture_names()
                .contains(&"variable")
        );
        assert!(
            typescript
                .highlights()
                .unwrap()
                .capture_names()
                .contains(&"type")
        );
        assert!(typescript.injections().is_some());

        let tsx = language_for_file(Path::new("view.tsx"), None).unwrap();
        assert!(
            tsx.highlights()
                .unwrap()
                .capture_names()
                .contains(&"variable")
        );
        assert!(
            tsx.highlights()
                .unwrap()
                .capture_names()
                .contains(&"tag.jsx")
        );
        assert!(tsx.highlights().unwrap().capture_names().contains(&"type"));
        assert!(tsx.injections().is_some());
    }

    #[test]
    fn builtin_language_specs_are_complete_unique_and_reachable() {
        let registry = registry();
        let mut plain_text_count = 0;
        let mut names = HashSet::new();
        let mut suffixes = HashSet::new();
        for spec in &registry.languages {
            assert!(
                names.insert(spec.name.to_ascii_lowercase()),
                "语言名 `{}` 不能重复",
                spec.name
            );
            for suffix in spec.matcher.suffixes {
                assert!(
                    suffixes.insert(suffix.to_ascii_lowercase()),
                    "文件后缀 `{suffix}` 不能由多个语言规格声明"
                );
            }
            assert!(
                !spec.matcher.suffixes.is_empty()
                    || spec.matcher.first_line_pattern.is_some()
                    || spec.injection_alias.is_some(),
                "{} 必须能通过文件、首行或注入别名到达",
                spec.name
            );

            let language = registry.load(spec);
            match spec.support {
                LanguageSupportSpec::PlainText => {
                    plain_text_count += 1;
                    assert!(language.grammar().is_none());
                    assert!(language.highlights().is_none());
                }
                LanguageSupportSpec::TreeSitter { .. } => {
                    assert!(
                        language.grammar().is_some(),
                        "{} 必须加载 grammar",
                        spec.name
                    );
                    assert!(
                        language.highlights().is_some(),
                        "{} 必须加载高亮查询",
                        spec.name
                    );
                    assert!(
                        !language.capture_names().is_empty(),
                        "{} 的高亮查询必须声明 capture",
                        spec.name
                    );
                    if !spec.matcher.suffixes.is_empty()
                        || spec.matcher.first_line_pattern.is_some()
                    {
                        assert!(
                            language.brackets().is_some(),
                            "{} 必须提供括号查询",
                            spec.name
                        );
                        assert!(
                            language.indents().is_some(),
                            "{} 必须提供缩进查询",
                            spec.name
                        );
                        assert!(language.folds().is_some(), "{} 必须提供折叠查询", spec.name);
                    }
                }
            }
        }
        assert_eq!(plain_text_count, 1, "注册表只能有一个纯文本兜底规格");
    }

    #[test]
    fn loaded_languages_declare_input_autoclose_pairs() {
        for path in ["main.rs", "main.py", "data.json", "README.md", "style.css"] {
            let language = language_for_file(Path::new(path), None)
                .unwrap_or_else(|| panic!("{path} 应加载语言"));
            let pairs = language.auto_close_pairs();
            assert!(!pairs.is_empty(), "{path} 应声明输入自动闭合配对");
            assert!(
                pairs
                    .iter()
                    .any(|pair| pair.start == "(" && pair.end == ")"),
                "{path} 应含括号对"
            );
        }
    }
}
