//! 语言注册与查询。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use tree_sitter::Query;
use zcv_text::WordBoundaryPolicy;

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
    word_characters: &'static str,
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
    outline: Option<Arc<Query>>,
    locals: Option<Arc<Query>>,
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

    pub(crate) fn outline(&self) -> Option<&Arc<Query>> {
        match &self.syntax {
            LanguageSyntax::PlainText => None,
            LanguageSyntax::TreeSitter { queries, .. } => queries.outline.as_ref(),
        }
    }

    pub(crate) fn locals(&self) -> Option<&Arc<Query>> {
        match &self.syntax {
            LanguageSyntax::PlainText => None,
            LanguageSyntax::TreeSitter { queries, .. } => queries.locals.as_ref(),
        }
    }

    /// 输入级自动闭合配对表（编辑器输入行为的数据源）。
    pub fn auto_close_pairs(&self) -> &'static [AutoClosePair] {
        self.auto_close_pairs
    }

    /// 本语言的词边界分类策略（对齐 Zed 的 `LanguageConfig::word_characters`）。
    pub fn word_boundary(&self) -> WordBoundaryPolicy {
        WordBoundaryPolicy {
            word_characters: self.word_characters,
        }
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
            word_characters: self.word_characters,
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
        outline: compile_optional_query(language_name, "大纲", grammar, sources.outline),
        locals: compile_optional_query(language_name, "局部语义", grammar, sources.locals),
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
///
/// 由应用装配层创建一次并以 `Arc` 显式注入 `LanguageBuffer`/`SyntaxMap`，不提供全局单例。
pub struct LanguageRegistry {
    languages: Vec<LanguageSpec>,
    loaded_languages: Mutex<HashMap<&'static str, Arc<Language>>>,
}

impl LanguageRegistry {
    pub fn new() -> Self {
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
    pub fn language_for_file(
        &self,
        path: &Path,
        first_line: Option<&str>,
    ) -> Option<Arc<Language>> {
        self.matched_language(path, first_line)
            .map(|entry| self.load(entry))
    }

    /// 按语言展示名、文件扩展名或注入别名选择语言。
    ///
    /// 围栏代码块使用语言名而非文件路径；
    /// 该入口让预览、文档等消费者与文件识别共享同一份语言注册表，而不是各自维护别名映射。
    pub(crate) fn language_for_name_or_extension(&self, name: &str) -> Option<Arc<Language>> {
        let name = name.trim().trim_start_matches('.');
        if name.is_empty() {
            return None;
        }
        self.languages
            .iter()
            .find(|entry| {
                entry.name.eq_ignore_ascii_case(name)
                    || entry
                        .matcher
                        .suffixes
                        .iter()
                        .any(|suffix| suffix.eq_ignore_ascii_case(name))
                    || entry
                        .injection_alias
                        .is_some_and(|alias| alias.eq_ignore_ascii_case(name))
            })
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

impl Default for LanguageRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "test/registry_tests.rs"]
mod tests;
