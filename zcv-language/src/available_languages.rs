//! 内置语言数据与查询源（对齐 Zed `available_languages`）。
//!
//! 新增语言只需在此数据表加一条 `LanguageEntry`；查询源与编译逻辑在此维护。

use std::sync::Arc;

use tree_sitter::Query;

use crate::registry::{LanguageEntry, LanguageMatcher, LanguageQueries};

/// tree-sitter 本身不提供查询继承；语言规格在加载时将基础查询与扩展查询合并编译。
#[derive(Clone, Copy)]
pub(crate) enum QuerySource {
    Single(&'static str),
    Combined(&'static [&'static str]),
}

impl QuerySource {
    pub(crate) fn compile(
        self,
        grammar: &tree_sitter::Language,
    ) -> Result<Query, tree_sitter::QueryError> {
        match self {
            Self::Single(source) => Query::new(grammar, source),
            Self::Combined(sources) => Query::new(grammar, &sources.join("\n")),
        }
    }
}

/// 一门语言的结构查询源（不含高亮/注入，二者在语言规格声明中）。
///
/// 与语言声明（[`LanguageEntry`]）同表维护：新增语言只改数据表一处；
/// 查询源缺失的语言加载为空查询集。
#[derive(Clone, Copy)]
pub(crate) struct LanguageQuerySources {
    brackets: Option<&'static str>,
    indents: Option<&'static str>,
    fold: Option<&'static str>,
}

impl LanguageQuerySources {
    pub(crate) fn from_sources(
        brackets: &'static str,
        indents: &'static str,
        fold: Option<&'static str>,
    ) -> Self {
        Self {
            brackets: Some(brackets),
            indents: Some(indents),
            fold,
        }
    }

    /// 编译为运行时查询集（语言加载时调用）。
    pub(crate) fn compile_all(self, grammar: &tree_sitter::Language) -> LanguageQueries {
        LanguageQueries {
            brackets: compile_query(grammar, self.brackets),
            indents: compile_query(grammar, self.indents),
            folds: compile_query(grammar, self.fold),
        }
    }
}

/// 编译单个结构查询；源缺失时为空（不参与该结构能力）。
fn compile_query(grammar: &tree_sitter::Language, source: Option<&str>) -> Option<Arc<Query>> {
    source.map(|source| {
        Arc::new(
            Query::new(grammar, source)
                .unwrap_or_else(|error| panic!("Zed tree-sitter 查询与 grammar 不匹配：{error}")),
        )
    })
}

/// 首行识别模式（对齐 Zed `LanguageMatcher` 的 `Option<Regex>` 形态）。
///
/// 内置模式静态正确，编译失败直接 panic（与查询编译的失败处理一致）。
fn first_line(pattern: &'static str) -> Option<regex::Regex> {
    Some(regex::Regex::new(pattern).expect("内置首行模式应有效"))
}

/// 内置语言数据：注册表初始化来源（新增语言在此加一条即可）。
pub(crate) fn builtin_languages() -> Vec<LanguageEntry> {
    use LanguageMatcher as M;
    vec![
        LanguageEntry {
            name: "Rust",
            matcher: M {
                suffixes: &["rs", "rlib"],
                first_line_pattern: None,
            },
            grammar: Some(|| tree_sitter_rust::LANGUAGE.into()),
            highlights: Some(QuerySource::Single(tree_sitter_rust::HIGHLIGHTS_QUERY)),
            injections: Some(QuerySource::Single(tree_sitter_rust::INJECTIONS_QUERY)),
            injection_alias: None,
            queries: Some(LanguageQuerySources::from_sources(
                include_str!("../queries/rust/brackets.scm"),
                include_str!("../queries/rust/indents.scm"),
                Some(include_str!("../queries/rust/fold.scm")),
            )),
        },
        LanguageEntry {
            name: "Python",
            matcher: M {
                suffixes: &["py", "pyw", "pyx"],
                first_line_pattern: first_line(
                    r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?python[\d.]*",
                ),
            },
            grammar: Some(|| tree_sitter_python::LANGUAGE.into()),
            highlights: Some(QuerySource::Single(tree_sitter_python::HIGHLIGHTS_QUERY)),
            injections: None,
            injection_alias: None,
            queries: Some(LanguageQuerySources::from_sources(
                include_str!("../queries/python/brackets.scm"),
                include_str!("../queries/python/indents.scm"),
                None,
            )),
        },
        LanguageEntry {
            name: "JavaScript",
            matcher: M {
                suffixes: &["js", "mjs", "cjs"],
                first_line_pattern: first_line(
                    r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?(?:node|bun|deno)",
                ),
            },
            grammar: Some(|| tree_sitter_javascript::LANGUAGE.into()),
            highlights: Some(QuerySource::Single(tree_sitter_javascript::HIGHLIGHT_QUERY)),
            injections: Some(QuerySource::Single(
                tree_sitter_javascript::INJECTIONS_QUERY,
            )),
            injection_alias: None,
            queries: Some(LanguageQuerySources::from_sources(
                include_str!("../queries/javascript/brackets.scm"),
                include_str!("../queries/javascript/indents.scm"),
                None,
            )),
        },
        LanguageEntry {
            name: "JSX",
            matcher: M {
                suffixes: &["jsx"],
                first_line_pattern: None,
            },
            grammar: Some(|| tree_sitter_typescript::LANGUAGE_TSX.into()),
            highlights: Some(QuerySource::Combined(&[
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
            ])),
            injections: Some(QuerySource::Single(
                tree_sitter_javascript::INJECTIONS_QUERY,
            )),
            injection_alias: None,
            queries: Some(LanguageQuerySources::from_sources(
                include_str!("../queries/tsx/brackets.scm"),
                include_str!("../queries/tsx/indents.scm"),
                None,
            )),
        },
        LanguageEntry {
            name: "TypeScript",
            matcher: M {
                suffixes: &["ts", "mts", "cts"],
                first_line_pattern: None,
            },
            grammar: Some(|| tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
            highlights: Some(QuerySource::Combined(&[
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
            ])),
            injections: Some(QuerySource::Single(
                tree_sitter_javascript::INJECTIONS_QUERY,
            )),
            injection_alias: None,
            queries: Some(LanguageQuerySources::from_sources(
                include_str!("../queries/typescript/brackets.scm"),
                include_str!("../queries/typescript/indents.scm"),
                None,
            )),
        },
        LanguageEntry {
            name: "TSX",
            matcher: M {
                suffixes: &["tsx"],
                first_line_pattern: None,
            },
            grammar: Some(|| tree_sitter_typescript::LANGUAGE_TSX.into()),
            highlights: Some(QuerySource::Combined(&[
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
            ])),
            injections: Some(QuerySource::Single(
                tree_sitter_javascript::INJECTIONS_QUERY,
            )),
            injection_alias: None,
            queries: Some(LanguageQuerySources::from_sources(
                include_str!("../queries/tsx/brackets.scm"),
                include_str!("../queries/tsx/indents.scm"),
                None,
            )),
        },
        LanguageEntry {
            name: "Go",
            matcher: M {
                suffixes: &["go"],
                first_line_pattern: None,
            },
            grammar: None,
            highlights: None,
            injections: None,
            injection_alias: None,
            queries: None,
        },
        LanguageEntry {
            name: "Java",
            matcher: M {
                suffixes: &["java"],
                first_line_pattern: None,
            },
            grammar: Some(|| tree_sitter_java::LANGUAGE.into()),
            highlights: Some(QuerySource::Single(tree_sitter_java::HIGHLIGHTS_QUERY)),
            injections: None,
            injection_alias: None,
            queries: None,
        },
        LanguageEntry {
            name: "Ruby",
            matcher: M {
                suffixes: &["rb", "erb"],
                first_line_pattern: first_line(
                    r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?ruby",
                ),
            },
            grammar: None,
            highlights: None,
            injections: None,
            injection_alias: None,
            queries: None,
        },
        LanguageEntry {
            name: "C",
            matcher: M {
                suffixes: &["c", "h"],
                first_line_pattern: None,
            },
            grammar: None,
            highlights: None,
            injections: None,
            injection_alias: None,
            queries: None,
        },
        LanguageEntry {
            name: "C++",
            matcher: M {
                suffixes: &["cpp", "cc", "cxx", "c++", "hpp", "hh", "hxx"],
                first_line_pattern: None,
            },
            grammar: None,
            highlights: None,
            injections: None,
            injection_alias: None,
            queries: None,
        },
        LanguageEntry {
            name: "Zig",
            matcher: M {
                suffixes: &["zig"],
                first_line_pattern: None,
            },
            grammar: None,
            highlights: None,
            injections: None,
            injection_alias: None,
            queries: None,
        },
        LanguageEntry {
            name: "Shell",
            matcher: M {
                suffixes: &["sh", "bash", "zsh", "ksh"],
                first_line_pattern: first_line(
                    r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?(?:bash|sh|zsh|ksh|dash)",
                ),
            },
            grammar: Some(|| tree_sitter_bash::LANGUAGE.into()),
            highlights: Some(QuerySource::Single(tree_sitter_bash::HIGHLIGHT_QUERY)),
            injections: None,
            injection_alias: None,
            queries: Some(LanguageQuerySources::from_sources(
                include_str!("../queries/bash/brackets.scm"),
                include_str!("../queries/bash/indents.scm"),
                None,
            )),
        },
        LanguageEntry {
            name: "Lua",
            matcher: M {
                suffixes: &["lua"],
                first_line_pattern: first_line(
                    r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?lua",
                ),
            },
            grammar: None,
            highlights: None,
            injections: None,
            injection_alias: None,
            queries: None,
        },
        LanguageEntry {
            name: "TOML",
            matcher: M {
                suffixes: &["toml"],
                first_line_pattern: None,
            },
            grammar: Some(|| tree_sitter_toml_ng::LANGUAGE.into()),
            highlights: Some(QuerySource::Single(tree_sitter_toml_ng::HIGHLIGHTS_QUERY)),
            injections: None,
            injection_alias: None,
            queries: None,
        },
        LanguageEntry {
            name: "JSON",
            matcher: M {
                suffixes: &["json"],
                first_line_pattern: None,
            },
            grammar: Some(|| tree_sitter_json::LANGUAGE.into()),
            highlights: Some(QuerySource::Single(tree_sitter_json::HIGHLIGHTS_QUERY)),
            injections: None,
            injection_alias: None,
            queries: Some(LanguageQuerySources::from_sources(
                include_str!("../queries/json/brackets.scm"),
                include_str!("../queries/json/indents.scm"),
                None,
            )),
        },
        LanguageEntry {
            name: "YAML",
            matcher: M {
                suffixes: &["yaml", "yml"],
                first_line_pattern: None,
            },
            grammar: Some(|| tree_sitter_yaml::LANGUAGE.into()),
            highlights: Some(QuerySource::Single(tree_sitter_yaml::HIGHLIGHTS_QUERY)),
            injections: None,
            injection_alias: None,
            queries: Some(LanguageQuerySources {
                brackets: Some(include_str!("../queries/yaml/brackets.scm")),
                indents: None,
                fold: None,
            }),
        },
        LanguageEntry {
            name: "Markdown",
            matcher: M {
                suffixes: &["md", "markdown"],
                first_line_pattern: None,
            },
            grammar: Some(|| tree_sitter_md::LANGUAGE.into()),
            highlights: Some(QuerySource::Single(tree_sitter_md::HIGHLIGHT_QUERY_BLOCK)),
            injections: Some(QuerySource::Single(tree_sitter_md::INJECTION_QUERY_BLOCK)),
            injection_alias: None,
            queries: Some(LanguageQuerySources::from_sources(
                include_str!("../queries/markdown/brackets.scm"),
                include_str!("../queries/markdown/indents.scm"),
                None,
            )),
        },
        // markdown 行内注入：仅注入查询引用（`markdown_inline`），不参与文件识别。
        LanguageEntry {
            name: "Markdown Inline",
            matcher: M {
                suffixes: &[],
                first_line_pattern: None,
            },
            grammar: Some(|| tree_sitter_md::INLINE_LANGUAGE.into()),
            highlights: Some(QuerySource::Single(tree_sitter_md::HIGHLIGHT_QUERY_INLINE)),
            injections: Some(QuerySource::Single(tree_sitter_md::INJECTION_QUERY_INLINE)),
            injection_alias: Some("markdown_inline"),
            queries: None,
        },
        LanguageEntry {
            name: "HTML",
            matcher: M {
                suffixes: &["html", "htm", "xhtml"],
                first_line_pattern: None,
            },
            grammar: Some(|| tree_sitter_html::LANGUAGE.into()),
            highlights: Some(QuerySource::Single(tree_sitter_html::HIGHLIGHTS_QUERY)),
            injections: Some(QuerySource::Single(tree_sitter_html::INJECTIONS_QUERY)),
            injection_alias: None,
            queries: Some(LanguageQuerySources::from_sources(
                include_str!("../queries/html/brackets.scm"),
                include_str!("../queries/html/indents.scm"),
                None,
            )),
        },
        LanguageEntry {
            name: "CSS",
            matcher: M {
                suffixes: &["css", "scss", "less", "sass"],
                first_line_pattern: None,
            },
            grammar: Some(|| tree_sitter_css::LANGUAGE.into()),
            highlights: Some(QuerySource::Single(tree_sitter_css::HIGHLIGHTS_QUERY)),
            injections: None,
            injection_alias: None,
            queries: Some(LanguageQuerySources::from_sources(
                include_str!("../queries/css/brackets.scm"),
                include_str!("../queries/css/indents.scm"),
                None,
            )),
        },
        LanguageEntry {
            name: "SQL",
            matcher: M {
                suffixes: &["sql"],
                first_line_pattern: None,
            },
            grammar: None,
            highlights: None,
            injections: None,
            injection_alias: None,
            queries: None,
        },
        LanguageEntry {
            name: "LaTeX",
            matcher: M {
                suffixes: &["tex", "latex", "sty", "cls", "bib"],
                first_line_pattern: None,
            },
            grammar: None,
            highlights: None,
            injections: None,
            injection_alias: None,
            queries: None,
        },
        LanguageEntry {
            name: "XML",
            matcher: M {
                suffixes: &["xml", "xsd", "xsl", "plist", "svg"],
                first_line_pattern: None,
            },
            grammar: None,
            highlights: None,
            injections: None,
            injection_alias: None,
            queries: None,
        },
    ]
}
