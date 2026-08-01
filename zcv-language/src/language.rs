use std::path::Path;
use std::sync::Arc;

use tree_sitter::Query;

/// 一门可由 tree-sitter 解析和高亮的语言。
#[derive(Clone)]
pub struct Language {
    name: &'static str,
    grammar: tree_sitter::Language,
    highlights: Arc<Query>,
    injections: Option<Arc<Query>>,
    capture_names: Arc<[Arc<str>]>,
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

    pub(crate) fn capture_name(&self, index: u32) -> Option<Arc<str>> {
        self.capture_names.get(index as usize).cloned()
    }
}

struct LanguageSpec {
    name: &'static str,
    suffixes: &'static [&'static str],
    first_line_pattern: Option<&'static str>,
    grammar: Option<fn(&str) -> tree_sitter::Language>,
    highlights: Option<QuerySource>,
    injections: Option<QuerySource>,
}

/// tree-sitter 本身不提供查询继承；语言规格在加载时将基础查询与扩展查询合并编译。
#[derive(Clone, Copy)]
enum QuerySource {
    Single(&'static str),
    Combined(&'static [&'static str]),
}

impl QuerySource {
    fn compile(self, grammar: &tree_sitter::Language) -> Result<Query, tree_sitter::QueryError> {
        match self {
            Self::Single(source) => Query::new(grammar, source),
            Self::Combined(sources) => Query::new(grammar, &sources.join("\n")),
        }
    }
}

impl LanguageSpec {
    fn load(&self, suffix: &str) -> Option<Language> {
        let grammar = (self.grammar?)(suffix);
        let highlights = self.highlights?.compile(&grammar).ok()?;
        let injections = self
            .injections
            .map(|source| source.compile(&grammar))
            .transpose()
            .ok()?;
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
            capture_names,
        })
    }
}

pub(crate) fn language_for_injection(name: &str) -> Option<Language> {
    let name = name.trim().to_ascii_lowercase();
    if name == "markdown_inline" {
        let grammar: tree_sitter::Language = tree_sitter_md::INLINE_LANGUAGE.into();
        let highlights = Query::new(&grammar, tree_sitter_md::HIGHLIGHT_QUERY_INLINE).ok()?;
        let injections = Query::new(&grammar, tree_sitter_md::INJECTION_QUERY_INLINE).ok()?;
        let capture_names = highlights
            .capture_names()
            .iter()
            .map(|name| Arc::<str>::from(*name))
            .collect();
        return Some(Language {
            name: "Markdown Inline",
            grammar,
            highlights: Arc::new(highlights),
            injections: Some(Arc::new(injections)),
            capture_names,
        });
    }

    LANGUAGES
        .iter()
        .find(|spec| {
            spec.name.eq_ignore_ascii_case(&name)
                || spec.suffixes.iter().any(|suffix| *suffix == name)
        })
        .and_then(|spec| spec.load(&name))
}

static LANGUAGES: &[LanguageSpec] = &[
    LanguageSpec {
        name: "Rust",
        suffixes: &["rs", "rlib"],
        first_line_pattern: None,
        grammar: Some(|_| tree_sitter_rust::LANGUAGE.into()),
        highlights: Some(QuerySource::Single(tree_sitter_rust::HIGHLIGHTS_QUERY)),
        injections: Some(QuerySource::Single(tree_sitter_rust::INJECTIONS_QUERY)),
    },
    LanguageSpec {
        name: "Python",
        suffixes: &["py", "pyw", "pyx"],
        first_line_pattern: Some(
            r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?python[\d.]*",
        ),
        grammar: Some(|_| tree_sitter_python::LANGUAGE.into()),
        highlights: Some(QuerySource::Single(tree_sitter_python::HIGHLIGHTS_QUERY)),
        injections: None,
    },
    LanguageSpec {
        name: "JavaScript",
        suffixes: &["js", "mjs", "cjs"],
        first_line_pattern: Some(
            r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?(?:node|bun|deno)",
        ),
        grammar: Some(|_| tree_sitter_javascript::LANGUAGE.into()),
        highlights: Some(QuerySource::Single(tree_sitter_javascript::HIGHLIGHT_QUERY)),
        injections: Some(QuerySource::Single(
            tree_sitter_javascript::INJECTIONS_QUERY,
        )),
    },
    LanguageSpec {
        name: "JSX",
        suffixes: &["jsx"],
        first_line_pattern: None,
        grammar: Some(|_| tree_sitter_typescript::LANGUAGE_TSX.into()),
        highlights: Some(QuerySource::Combined(&[
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
        ])),
        injections: Some(QuerySource::Single(
            tree_sitter_javascript::INJECTIONS_QUERY,
        )),
    },
    LanguageSpec {
        name: "TypeScript",
        suffixes: &["ts", "mts", "cts"],
        first_line_pattern: None,
        grammar: Some(|_| tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        highlights: Some(QuerySource::Combined(&[
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
        ])),
        injections: Some(QuerySource::Single(
            tree_sitter_javascript::INJECTIONS_QUERY,
        )),
    },
    LanguageSpec {
        name: "TSX",
        suffixes: &["tsx"],
        first_line_pattern: None,
        grammar: Some(|_| tree_sitter_typescript::LANGUAGE_TSX.into()),
        highlights: Some(QuerySource::Combined(&[
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
        ])),
        injections: Some(QuerySource::Single(
            tree_sitter_javascript::INJECTIONS_QUERY,
        )),
    },
    LanguageSpec {
        name: "Go",
        suffixes: &["go"],
        first_line_pattern: None,
        grammar: None,
        highlights: None,
        injections: None,
    },
    LanguageSpec {
        name: "Java",
        suffixes: &["java"],
        first_line_pattern: None,
        grammar: Some(|_| tree_sitter_java::LANGUAGE.into()),
        highlights: Some(QuerySource::Single(tree_sitter_java::HIGHLIGHTS_QUERY)),
        injections: None,
    },
    LanguageSpec {
        name: "Ruby",
        suffixes: &["rb", "erb"],
        first_line_pattern: Some(r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?ruby"),
        grammar: None,
        highlights: None,
        injections: None,
    },
    LanguageSpec {
        name: "C",
        suffixes: &["c", "h"],
        first_line_pattern: None,
        grammar: None,
        highlights: None,
        injections: None,
    },
    LanguageSpec {
        name: "C++",
        suffixes: &["cpp", "cc", "cxx", "c++", "hpp", "hh", "hxx"],
        first_line_pattern: None,
        grammar: None,
        highlights: None,
        injections: None,
    },
    LanguageSpec {
        name: "Zig",
        suffixes: &["zig"],
        first_line_pattern: None,
        grammar: None,
        highlights: None,
        injections: None,
    },
    LanguageSpec {
        name: "Shell",
        suffixes: &["sh", "bash", "zsh", "ksh"],
        first_line_pattern: Some(
            r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?(?:bash|sh|zsh|ksh|dash)",
        ),
        grammar: Some(|_| tree_sitter_bash::LANGUAGE.into()),
        highlights: Some(QuerySource::Single(tree_sitter_bash::HIGHLIGHT_QUERY)),
        injections: None,
    },
    LanguageSpec {
        name: "Lua",
        suffixes: &["lua"],
        first_line_pattern: Some(r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?lua"),
        grammar: None,
        highlights: None,
        injections: None,
    },
    LanguageSpec {
        name: "TOML",
        suffixes: &["toml"],
        first_line_pattern: None,
        grammar: Some(|_| tree_sitter_toml_ng::LANGUAGE.into()),
        highlights: Some(QuerySource::Single(tree_sitter_toml_ng::HIGHLIGHTS_QUERY)),
        injections: None,
    },
    LanguageSpec {
        name: "JSON",
        suffixes: &["json"],
        first_line_pattern: None,
        grammar: Some(|_| tree_sitter_json::LANGUAGE.into()),
        highlights: Some(QuerySource::Single(tree_sitter_json::HIGHLIGHTS_QUERY)),
        injections: None,
    },
    LanguageSpec {
        name: "YAML",
        suffixes: &["yaml", "yml"],
        first_line_pattern: None,
        grammar: Some(|_| tree_sitter_yaml::LANGUAGE.into()),
        highlights: Some(QuerySource::Single(tree_sitter_yaml::HIGHLIGHTS_QUERY)),
        injections: None,
    },
    LanguageSpec {
        name: "Markdown",
        suffixes: &["md", "markdown"],
        first_line_pattern: None,
        grammar: Some(|_| tree_sitter_md::LANGUAGE.into()),
        highlights: Some(QuerySource::Single(tree_sitter_md::HIGHLIGHT_QUERY_BLOCK)),
        injections: Some(QuerySource::Single(tree_sitter_md::INJECTION_QUERY_BLOCK)),
    },
    LanguageSpec {
        name: "HTML",
        suffixes: &["html", "htm", "xhtml"],
        first_line_pattern: None,
        grammar: Some(|_| tree_sitter_html::LANGUAGE.into()),
        highlights: Some(QuerySource::Single(tree_sitter_html::HIGHLIGHTS_QUERY)),
        injections: Some(QuerySource::Single(tree_sitter_html::INJECTIONS_QUERY)),
    },
    LanguageSpec {
        name: "CSS",
        suffixes: &["css", "scss", "less", "sass"],
        first_line_pattern: None,
        grammar: Some(|_| tree_sitter_css::LANGUAGE.into()),
        highlights: Some(QuerySource::Single(tree_sitter_css::HIGHLIGHTS_QUERY)),
        injections: None,
    },
    LanguageSpec {
        name: "SQL",
        suffixes: &["sql"],
        first_line_pattern: None,
        grammar: None,
        highlights: None,
        injections: None,
    },
    LanguageSpec {
        name: "LaTeX",
        suffixes: &["tex", "latex", "sty", "cls", "bib"],
        first_line_pattern: None,
        grammar: None,
        highlights: None,
        injections: None,
    },
    LanguageSpec {
        name: "XML",
        suffixes: &["xml", "xsd", "xsl", "plist", "svg"],
        first_line_pattern: None,
        grammar: None,
        highlights: None,
        injections: None,
    },
];

/// 根据文件名和首行内容选择已注册且可高亮的语言。
pub(crate) fn language_for_file(path: &Path, first_line: Option<&str>) -> Option<Language> {
    matched_language(path, first_line).and_then(|(spec, suffix)| spec.load(suffix))
}

/// 返回语言显示名；即使暂未注册 grammar，也保留文件类型识别能力。
pub(crate) fn language_name_for_file(
    path: &Path,
    first_line: Option<&str>,
) -> Option<&'static str> {
    matched_language(path, first_line).map(|(spec, _)| spec.name)
}

fn matched_language(
    path: &Path,
    first_line: Option<&str>,
) -> Option<(&'static LanguageSpec, &'static str)> {
    let filename = path.file_name()?.to_str()?;
    let mut matched = LANGUAGES
        .iter()
        .flat_map(|language| {
            language
                .suffixes
                .iter()
                .filter(move |suffix| filename.ends_with(&format!(".{suffix}")))
                .map(move |suffix| (language, *suffix))
        })
        .max_by_key(|(_, suffix)| suffix.len());

    if matched.is_none()
        && let Some(first_line) = first_line
    {
        matched = LANGUAGES.iter().find_map(|language| {
            let pattern = language.first_line_pattern?;
            regex::Regex::new(pattern)
                .ok()?
                .is_match(first_line)
                .then_some((language, ""))
        });
    }

    matched
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
}
