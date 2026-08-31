//! 内置语言规格。
//!
//! 每门 Tree-sitter 语言在一个规格中声明识别规则、grammar、全部查询源和输入配置。
//! `highlights.scm` 是 Tree-sitter 支持的必需组成部分，其余查询按实际能力显式提供。

use crate::AutoClosePair;

/// 文件识别规则：扩展名 + 首行模式。
pub(crate) struct LanguageMatcher {
    pub(crate) suffixes: &'static [&'static str],
    pub(crate) first_line_pattern: Option<regex::Regex>,
}

/// 一门 Tree-sitter 语言的全部查询源。
///
/// 高亮查询是语法支持的必需能力；注入、括号、缩进和折叠查询按语言能力选择。
#[derive(Clone, Copy)]
pub(crate) struct LanguageQuerySources {
    pub(crate) highlights: &'static str,
    pub(crate) injections: Option<&'static str>,
    pub(crate) brackets: Option<&'static str>,
    pub(crate) indents: Option<&'static str>,
    pub(crate) folds: Option<&'static str>,
}

impl LanguageQuerySources {
    pub(crate) const fn new(highlights: &'static str) -> Self {
        Self {
            highlights,
            injections: None,
            brackets: None,
            indents: None,
            folds: None,
        }
    }

    pub(crate) const fn with_injections(mut self, source: &'static str) -> Self {
        self.injections = Some(source);
        self
    }

    pub(crate) const fn with_brackets(mut self, source: &'static str) -> Self {
        self.brackets = Some(source);
        self
    }

    pub(crate) const fn with_indents(mut self, source: &'static str) -> Self {
        self.indents = Some(source);
        self
    }

    pub(crate) const fn with_folds(mut self, source: &'static str) -> Self {
        self.folds = Some(source);
        self
    }
}

macro_rules! query_sources {
    ($language:literal) => {
        LanguageQuerySources::new(include_str!(concat!(
            "../queries/",
            $language,
            "/highlights.scm"
        )))
    };
}

/// 语言的语法支持类型。
///
/// 只有真正的纯文本可以没有 grammar；
/// Tree-sitter 语言必须携带 grammar 和完整查询规格。
pub(crate) enum LanguageSupport {
    PlainText,
    TreeSitter {
        grammar: fn() -> tree_sitter::Language,
        queries: LanguageQuerySources,
    },
}

/// 一门内置语言的唯一规格。
pub(crate) struct LanguageSpec {
    pub(crate) name: &'static str,
    pub(crate) matcher: LanguageMatcher,
    pub(crate) support: LanguageSupport,
    /// 注入查询使用的别名；只参与注入查找，不参与文件识别。
    pub(crate) injection_alias: Option<&'static str>,
    pub(crate) auto_close_pairs: &'static [AutoClosePair],
}

impl LanguageSpec {
    fn tree_sitter(
        name: &'static str,
        matcher: LanguageMatcher,
        grammar: fn() -> tree_sitter::Language,
        queries: LanguageQuerySources,
        injection_alias: Option<&'static str>,
        auto_close_pairs: &'static [AutoClosePair],
    ) -> Self {
        Self {
            name,
            matcher,
            support: LanguageSupport::TreeSitter { grammar, queries },
            injection_alias,
            auto_close_pairs,
        }
    }

    fn plain_text(name: &'static str, matcher: LanguageMatcher) -> Self {
        Self {
            name,
            matcher,
            support: LanguageSupport::PlainText,
            injection_alias: None,
            auto_close_pairs: &[],
        }
    }

    /// 注入名匹配：别名、语言名与文件后缀均忽略大小写。
    pub(crate) fn matches_injection_name(&self, name: &str) -> bool {
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

/// 首行识别模式。
///
/// 内置模式静态正确，编译失败直接终止初始化。
fn first_line(pattern: &'static str) -> Option<regex::Regex> {
    Some(regex::Regex::new(pattern).expect("内置首行模式应有效"))
}

const COMMON_PAIRS: &[AutoClosePair] = &[
    AutoClosePair {
        start: "(",
        end: ")",
        close: true,
        surround: true,
        newline: true,
    },
    AutoClosePair {
        start: "[",
        end: "]",
        close: true,
        surround: true,
        newline: true,
    },
    AutoClosePair {
        start: "{",
        end: "}",
        close: true,
        surround: true,
        newline: true,
    },
    AutoClosePair {
        start: "\"",
        end: "\"",
        close: true,
        surround: true,
        newline: false,
    },
    AutoClosePair {
        start: "'",
        end: "'",
        close: true,
        surround: true,
        newline: false,
    },
];

const COMMON_BRACKETS_QUERY: &str = include_str!("../queries/common/brackets.scm");

/// 所有内置语言规格。
///
/// 新增语言时在此登记一个完整规格；
/// 查询可以来自 grammar crate，也可以由 `queries/<language>/` 提供。
pub(crate) fn builtin_languages() -> Vec<LanguageSpec> {
    vec![
        LanguageSpec::tree_sitter(
            "Rust",
            LanguageMatcher {
                suffixes: &["rs", "rlib"],
                first_line_pattern: None,
            },
            || tree_sitter_rust::LANGUAGE.into(),
            query_sources!("rust")
                .with_injections(include_str!("../queries/rust/injections.scm"))
                .with_brackets(include_str!("../queries/rust/brackets.scm"))
                .with_indents(include_str!("../queries/rust/indents.scm"))
                .with_folds(include_str!("../queries/rust/fold.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "C",
            LanguageMatcher {
                suffixes: &["c"],
                first_line_pattern: None,
            },
            || tree_sitter_c::LANGUAGE.into(),
            LanguageQuerySources::new(tree_sitter_c::HIGHLIGHT_QUERY)
                .with_injections(include_str!("../queries/c/injections.scm"))
                .with_brackets(COMMON_BRACKETS_QUERY)
                .with_indents(include_str!("../queries/c/indents.scm"))
                .with_folds(include_str!("../queries/c/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "C++",
            LanguageMatcher {
                suffixes: &[
                    "cc", "ccm", "hh", "cpp", "cppm", "h", "hpp", "cxx", "cxxm", "hxx", "c++",
                    "c++m", "h++", "hip", "ipp", "inl", "ino", "ixx", "cu", "cuh",
                ],
                first_line_pattern: first_line(r"^//.*-\*-\s*C\+\+\s*-\*-"),
            },
            || tree_sitter_cpp::LANGUAGE.into(),
            query_sources!("cpp")
                .with_injections(include_str!("../queries/cpp/injections.scm"))
                .with_brackets(COMMON_BRACKETS_QUERY)
                .with_indents(include_str!("../queries/cpp/indents.scm"))
                .with_folds(include_str!("../queries/cpp/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "C#",
            LanguageMatcher {
                suffixes: &["cs", "csx"],
                first_line_pattern: None,
            },
            || tree_sitter_c_sharp::LANGUAGE.into(),
            LanguageQuerySources::new(tree_sitter_c_sharp::HIGHLIGHTS_QUERY)
                .with_brackets(COMMON_BRACKETS_QUERY)
                .with_indents(include_str!("../queries/c_sharp/indents.scm"))
                .with_folds(include_str!("../queries/c_sharp/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "Go",
            LanguageMatcher {
                suffixes: &["go"],
                first_line_pattern: first_line(r"^//.*\bgo run\b"),
            },
            || tree_sitter_go::LANGUAGE.into(),
            query_sources!("go")
                .with_brackets(COMMON_BRACKETS_QUERY)
                .with_indents(include_str!("../queries/go/indents.scm"))
                .with_folds(include_str!("../queries/go/folds.scm")),
            Some("golang"),
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "Python",
            LanguageMatcher {
                suffixes: &["py", "pyw", "pyx"],
                first_line_pattern: first_line(
                    r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?python[\d.]*",
                ),
            },
            || tree_sitter_python::LANGUAGE.into(),
            query_sources!("python")
                .with_injections(include_str!("../queries/python/injections.scm"))
                .with_brackets(include_str!("../queries/python/brackets.scm"))
                .with_indents(include_str!("../queries/python/indents.scm"))
                .with_folds(include_str!("../queries/python/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "JavaScript",
            LanguageMatcher {
                suffixes: &["js", "mjs", "cjs"],
                first_line_pattern: first_line(
                    r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?(?:node|bun|deno)",
                ),
            },
            || tree_sitter_typescript::LANGUAGE_TSX.into(),
            query_sources!("javascript")
                .with_injections(include_str!("../queries/javascript/injections.scm"))
                .with_brackets(include_str!("../queries/javascript/brackets.scm"))
                .with_indents(include_str!("../queries/javascript/indents.scm"))
                .with_folds(include_str!("../queries/javascript/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "JSX",
            LanguageMatcher {
                suffixes: &["jsx"],
                first_line_pattern: None,
            },
            || tree_sitter_typescript::LANGUAGE_TSX.into(),
            query_sources!("javascript")
                .with_injections(include_str!("../queries/javascript/injections.scm"))
                .with_brackets(include_str!("../queries/tsx/brackets.scm"))
                .with_indents(include_str!("../queries/tsx/indents.scm"))
                .with_folds(include_str!("../queries/javascript/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "TypeScript",
            LanguageMatcher {
                suffixes: &["ts", "mts", "cts"],
                first_line_pattern: None,
            },
            || tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            query_sources!("typescript")
                .with_injections(include_str!("../queries/typescript/injections.scm"))
                .with_brackets(include_str!("../queries/typescript/brackets.scm"))
                .with_indents(include_str!("../queries/typescript/indents.scm"))
                .with_folds(include_str!("../queries/javascript/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "TSX",
            LanguageMatcher {
                suffixes: &["tsx"],
                first_line_pattern: None,
            },
            || tree_sitter_typescript::LANGUAGE_TSX.into(),
            query_sources!("tsx")
                .with_injections(include_str!("../queries/tsx/injections.scm"))
                .with_brackets(include_str!("../queries/tsx/brackets.scm"))
                .with_indents(include_str!("../queries/tsx/indents.scm"))
                .with_folds(include_str!("../queries/javascript/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "Java",
            LanguageMatcher {
                suffixes: &["java"],
                first_line_pattern: None,
            },
            || tree_sitter_java::LANGUAGE.into(),
            query_sources!("java")
                .with_brackets(COMMON_BRACKETS_QUERY)
                .with_indents(include_str!("../queries/java/indents.scm"))
                .with_folds(include_str!("../queries/java/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "Kotlin",
            LanguageMatcher {
                suffixes: &["kt", "kts"],
                first_line_pattern: None,
            },
            || tree_sitter_kotlin_ng::LANGUAGE.into(),
            query_sources!("kotlin")
                .with_brackets(COMMON_BRACKETS_QUERY)
                .with_indents(include_str!("../queries/kotlin/indents.scm"))
                .with_folds(include_str!("../queries/kotlin/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "Shell",
            LanguageMatcher {
                suffixes: &["sh", "bash", "zsh", "ksh"],
                first_line_pattern: first_line(
                    r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?(?:bash|sh|zsh|ksh|dash)",
                ),
            },
            || tree_sitter_bash::LANGUAGE.into(),
            query_sources!("bash")
                .with_brackets(include_str!("../queries/bash/brackets.scm"))
                .with_indents(include_str!("../queries/bash/indents.scm"))
                .with_folds(include_str!("../queries/bash/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "Ruby",
            LanguageMatcher {
                suffixes: &["rb", "rake", "gemspec"],
                first_line_pattern: first_line(
                    r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?ruby(?:\s|$)",
                ),
            },
            || tree_sitter_ruby::LANGUAGE.into(),
            LanguageQuerySources::new(tree_sitter_ruby::HIGHLIGHTS_QUERY)
                .with_brackets(COMMON_BRACKETS_QUERY)
                .with_indents(include_str!("../queries/ruby/indents.scm"))
                .with_folds(include_str!("../queries/ruby/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "PHP",
            LanguageMatcher {
                suffixes: &["php", "php3", "php4", "php5", "php7", "php8", "phtml"],
                first_line_pattern: first_line(
                    r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?php(?:\s|$)",
                ),
            },
            || tree_sitter_php::LANGUAGE_PHP.into(),
            LanguageQuerySources::new(tree_sitter_php::HIGHLIGHTS_QUERY)
                .with_injections(tree_sitter_php::INJECTIONS_QUERY)
                .with_brackets(COMMON_BRACKETS_QUERY)
                .with_indents(include_str!("../queries/php/indents.scm"))
                .with_folds(include_str!("../queries/php/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "Swift",
            LanguageMatcher {
                suffixes: &["swift"],
                first_line_pattern: first_line(
                    r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?swift(?:\s|$)",
                ),
            },
            || tree_sitter_swift::LANGUAGE.into(),
            LanguageQuerySources::new(tree_sitter_swift::HIGHLIGHTS_QUERY)
                .with_brackets(COMMON_BRACKETS_QUERY)
                .with_indents(include_str!("../queries/swift/indents.scm"))
                .with_folds(include_str!("../queries/swift/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "Lua",
            LanguageMatcher {
                suffixes: &["lua"],
                first_line_pattern: first_line(
                    r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?lua[\d.]*",
                ),
            },
            || tree_sitter_lua::LANGUAGE.into(),
            LanguageQuerySources::new(tree_sitter_lua::HIGHLIGHTS_QUERY)
                .with_injections(tree_sitter_lua::INJECTIONS_QUERY)
                .with_brackets(COMMON_BRACKETS_QUERY)
                .with_indents(include_str!("../queries/lua/indents.scm"))
                .with_folds(include_str!("../queries/lua/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "Zig",
            LanguageMatcher {
                suffixes: &["zig"],
                first_line_pattern: None,
            },
            || tree_sitter_zig::LANGUAGE.into(),
            LanguageQuerySources::new(tree_sitter_zig::HIGHLIGHTS_QUERY)
                .with_brackets(COMMON_BRACKETS_QUERY)
                .with_indents(include_str!("../queries/zig/indents.scm"))
                .with_folds(include_str!("../queries/zig/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "SQL",
            LanguageMatcher {
                suffixes: &["sql"],
                first_line_pattern: None,
            },
            || tree_sitter_sequel::LANGUAGE.into(),
            query_sources!("sql")
                .with_brackets(include_str!("../queries/sql/brackets.scm"))
                .with_indents(include_str!("../queries/sql/indents.scm"))
                .with_folds(include_str!("../queries/sql/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "TOML",
            LanguageMatcher {
                suffixes: &["toml"],
                first_line_pattern: None,
            },
            || tree_sitter_toml_ng::LANGUAGE.into(),
            query_sources!("toml")
                .with_brackets(include_str!("../queries/toml/brackets.scm"))
                .with_indents(include_str!("../queries/toml/indents.scm"))
                .with_folds(include_str!("../queries/toml/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "JSON",
            LanguageMatcher {
                suffixes: &["json"],
                first_line_pattern: None,
            },
            || tree_sitter_json::LANGUAGE.into(),
            query_sources!("json")
                .with_brackets(include_str!("../queries/json/brackets.scm"))
                .with_indents(include_str!("../queries/json/indents.scm"))
                .with_folds(include_str!("../queries/json/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "YAML",
            LanguageMatcher {
                suffixes: &["yaml", "yml"],
                first_line_pattern: None,
            },
            || tree_sitter_yaml::LANGUAGE.into(),
            query_sources!("yaml")
                .with_injections(include_str!("../queries/yaml/injections.scm"))
                .with_brackets(include_str!("../queries/yaml/brackets.scm"))
                .with_indents(include_str!("../queries/yaml/indents.scm"))
                .with_folds(include_str!("../queries/yaml/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "Markdown",
            LanguageMatcher {
                suffixes: &["md", "markdown"],
                first_line_pattern: None,
            },
            || tree_sitter_md::LANGUAGE.into(),
            query_sources!("markdown")
                .with_injections(include_str!("../queries/markdown/injections.scm"))
                .with_brackets(include_str!("../queries/markdown/brackets.scm"))
                .with_indents(include_str!("../queries/markdown/indents.scm"))
                .with_folds(include_str!("../queries/markdown/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "Markdown Inline",
            LanguageMatcher {
                suffixes: &[],
                first_line_pattern: None,
            },
            || tree_sitter_md::INLINE_LANGUAGE.into(),
            query_sources!("markdown_inline")
                .with_injections(include_str!("../queries/markdown_inline/injections.scm")),
            Some("markdown_inline"),
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "HTML",
            LanguageMatcher {
                suffixes: &["html", "htm", "xhtml"],
                first_line_pattern: None,
            },
            || tree_sitter_html::LANGUAGE.into(),
            query_sources!("html")
                .with_injections(include_str!("../queries/html/injections.scm"))
                .with_brackets(include_str!("../queries/html/brackets.scm"))
                .with_indents(include_str!("../queries/html/indents.scm"))
                .with_folds(include_str!("../queries/html/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::tree_sitter(
            "CSS",
            LanguageMatcher {
                suffixes: &["css", "scss", "less", "sass"],
                first_line_pattern: None,
            },
            || tree_sitter_css::LANGUAGE.into(),
            query_sources!("css")
                .with_brackets(include_str!("../queries/css/brackets.scm"))
                .with_indents(include_str!("../queries/css/indents.scm"))
                .with_folds(include_str!("../queries/css/folds.scm")),
            None,
            COMMON_PAIRS,
        ),
        LanguageSpec::plain_text(
            "纯文本",
            LanguageMatcher {
                suffixes: &["txt", "text"],
                first_line_pattern: None,
            },
        ),
    ]
}
