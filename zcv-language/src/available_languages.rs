//! 内置语言规格。
//!
//! 每门 Tree-sitter 语言在一个规格中声明识别规则、grammar、全部查询源和输入配置。
//! 可直接识别文件的语言必须提供高亮、括号、缩进和折叠查询；
//! 注入查询只在语言存在真实嵌套语义时提供。

use crate::{
    AutoClosePair, BlockCommentConfig, LanguageInputConfig, OrderedListConfig, TaskListConfig,
};

/// 文件识别规则：扩展名 + 首行模式。
pub(crate) struct LanguageMatcher {
    pub(crate) suffixes: &'static [&'static str],
    pub(crate) first_line_pattern: Option<regex::Regex>,
}

/// 一门 Tree-sitter 语言的全部查询源。
///
/// 高亮查询是语法支持的必需能力；文件语言必须提供括号、缩进和折叠查询，
/// 内嵌辅助语言可省略结构查询，注入查询则按真实嵌套语义选择。
#[derive(Clone, Copy)]
pub(crate) struct LanguageQuerySources {
    pub(crate) highlights: &'static str,
    pub(crate) injections: Option<&'static str>,
    pub(crate) brackets: Option<&'static str>,
    pub(crate) indents: Option<&'static str>,
    pub(crate) folds: Option<&'static str>,
    pub(crate) outline: Option<&'static str>,
    pub(crate) locals: Option<&'static str>,
    pub(crate) overrides: Option<&'static str>,
}

impl LanguageQuerySources {
    pub(crate) const fn file_language(
        highlights: &'static str,
        brackets: &'static str,
        indents: &'static str,
        folds: &'static str,
    ) -> Self {
        Self {
            highlights,
            injections: None,
            brackets: Some(brackets),
            indents: Some(indents),
            folds: Some(folds),
            outline: None,
            locals: None,
            overrides: None,
        }
    }

    pub(crate) const fn embedded(highlights: &'static str) -> Self {
        Self {
            highlights,
            injections: None,
            brackets: None,
            indents: None,
            folds: None,
            outline: None,
            locals: None,
            overrides: None,
        }
    }

    pub(crate) const fn with_injections(mut self, source: &'static str) -> Self {
        self.injections = Some(source);
        self
    }

    pub(crate) const fn with_outline(mut self, source: &'static str) -> Self {
        self.outline = Some(source);
        self
    }

    pub(crate) const fn with_locals(mut self, source: &'static str) -> Self {
        self.locals = Some(source);
        self
    }

    pub(crate) const fn with_overrides(mut self, source: &'static str) -> Self {
        self.overrides = Some(source);
        self
    }
}

macro_rules! file_language_queries {
    ($language:literal) => {
        file_language_queries!(
            $language,
            include_str!(concat!("../queries/", $language, "/highlights.scm"))
        )
    };
    ($language:literal, $highlights:expr) => {
        LanguageQuerySources::file_language(
            $highlights,
            include_str!(concat!("../queries/", $language, "/brackets.scm")),
            include_str!(concat!("../queries/", $language, "/indents.scm")),
            include_str!(concat!("../queries/", $language, "/folds.scm")),
        )
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
    pub(crate) autoclose_before: &'static str,
    pub(crate) input: LanguageInputConfig,
    /// 除字母数字与 `_` 外，本语言额外视为词字符的字符集合（对齐 Zed 的 word_characters）。
    pub(crate) word_characters: &'static str,
}

impl LanguageSpec {
    fn tree_sitter(
        name: &'static str,
        matcher: LanguageMatcher,
        grammar: fn() -> tree_sitter::Language,
        queries: LanguageQuerySources,
        injection_alias: Option<&'static str>,
        auto_close_pairs: &'static [AutoClosePair],
        autoclose_before: &'static str,
    ) -> Self {
        Self {
            name,
            matcher,
            support: LanguageSupport::TreeSitter { grammar, queries },
            injection_alias,
            auto_close_pairs,
            autoclose_before,
            input: LanguageInputConfig::empty(),
            word_characters: "",
        }
    }

    fn plain_text(name: &'static str, matcher: LanguageMatcher) -> Self {
        Self {
            name,
            matcher,
            support: LanguageSupport::PlainText,
            injection_alias: None,
            auto_close_pairs: &[],
            autoclose_before: "",
            input: LanguageInputConfig::empty(),
            word_characters: "",
        }
    }

    /// 声明语言的额外词字符（对齐 Zed `LanguageConfig::word_characters`）。
    fn with_word_characters(mut self, word_characters: &'static str) -> Self {
        self.word_characters = word_characters;
        self
    }

    /// 声明语言的注释与列表换行政策。
    fn with_input(mut self, input: LanguageInputConfig) -> Self {
        self.input = input;
        self
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
        not_in: &[],
    },
    AutoClosePair {
        start: "[",
        end: "]",
        close: true,
        surround: true,
        newline: true,
        not_in: &[],
    },
    AutoClosePair {
        start: "{",
        end: "}",
        close: true,
        surround: true,
        newline: true,
        not_in: &[],
    },
    AutoClosePair {
        start: "\"",
        end: "\"",
        close: true,
        surround: true,
        newline: false,
        not_in: &[],
    },
    AutoClosePair {
        start: "'",
        end: "'",
        close: true,
        surround: true,
        newline: false,
        not_in: &[],
    },
];

const DEFAULT_INPUT_FOLLOWERS: &str = ";:.,=}])>";
const SHELL_INPUT_FOLLOWERS: &str = "}])";
const JSON_INPUT_FOLLOWERS: &str = ",]}";

const RUST_LINE_COMMENTS: &[&str] = &["// ", "/// ", "//! "];
const SCRIPT_LINE_COMMENTS: &[&str] = &["// "];
const HASH_LINE_COMMENTS: &[&str] = &["# "];
const C_LINE_COMMENTS: &[&str] = &["// "];
const CPP_LINE_COMMENTS: &[&str] = &["// ", "/// ", "//! "];
const JSON_LINE_COMMENTS: &[&str] = &["// "];

const RUST_BLOCK_COMMENT: BlockCommentConfig = BlockCommentConfig {
    start: "/*",
    prefix: "* ",
    end: "*/",
    tab_size: 1,
};
const SCRIPT_BLOCK_COMMENT: BlockCommentConfig = BlockCommentConfig {
    start: "/*",
    prefix: "* ",
    end: "*/",
    tab_size: 1,
};
const SCRIPT_DOC_COMMENT: BlockCommentConfig = BlockCommentConfig {
    start: "/**",
    prefix: "* ",
    end: "*/",
    tab_size: 1,
};
const MARKDOWN_BLOCK_COMMENT: BlockCommentConfig = BlockCommentConfig {
    start: "<!--",
    prefix: "",
    end: "-->",
    tab_size: 1,
};
const PYTHON_BLOCK_COMMENT: BlockCommentConfig = BlockCommentConfig {
    start: "\"\"\"",
    prefix: "",
    end: "\"\"\"",
    tab_size: 1,
};
const C_BLOCK_COMMENT: BlockCommentConfig = BlockCommentConfig {
    start: "/*",
    prefix: "",
    end: "*/",
    tab_size: 1,
};
const C_DOC_COMMENT: BlockCommentConfig = BlockCommentConfig {
    start: "/*",
    prefix: "* ",
    end: "*/",
    tab_size: 1,
};

const MARKDOWN_UNORDERED_LISTS: &[&str] = &["- ", "* ", "+ "];
const MARKDOWN_ORDERED_LISTS: &[OrderedListConfig] = &[OrderedListConfig {
    pattern: r"(\d+)\. ",
    format: "{1}. ",
}];
const MARKDOWN_TASK_LIST: TaskListConfig = TaskListConfig {
    prefixes: &["- [ ] ", "- [x] ", "- [X] "],
    continuation: "- [ ] ",
};

const RUST_INPUT: LanguageInputConfig = LanguageInputConfig {
    line_comments: RUST_LINE_COMMENTS,
    block_comment: Some(RUST_BLOCK_COMMENT),
    documentation_comment: Some(RUST_BLOCK_COMMENT),
    ..LanguageInputConfig::empty()
};
const SCRIPT_INPUT: LanguageInputConfig = LanguageInputConfig {
    line_comments: SCRIPT_LINE_COMMENTS,
    block_comment: Some(SCRIPT_BLOCK_COMMENT),
    documentation_comment: Some(SCRIPT_DOC_COMMENT),
    ..LanguageInputConfig::empty()
};
const JSX_INPUT: LanguageInputConfig = LanguageInputConfig {
    element_block_comment: Some(BlockCommentConfig {
        start: "{/*",
        prefix: "",
        end: "*/}",
        tab_size: 1,
    }),
    ..SCRIPT_INPUT
};
const MARKDOWN_INPUT: LanguageInputConfig = LanguageInputConfig {
    block_comment: Some(MARKDOWN_BLOCK_COMMENT),
    unordered_list: MARKDOWN_UNORDERED_LISTS,
    ordered_list: MARKDOWN_ORDERED_LISTS,
    task_list: Some(MARKDOWN_TASK_LIST),
    ..LanguageInputConfig::empty()
};
const SHELL_INPUT: LanguageInputConfig = LanguageInputConfig {
    line_comments: HASH_LINE_COMMENTS,
    ..LanguageInputConfig::empty()
};
const JSON_INPUT: LanguageInputConfig = LanguageInputConfig {
    line_comments: JSON_LINE_COMMENTS,
    ..LanguageInputConfig::empty()
};
const YAML_INPUT: LanguageInputConfig = LanguageInputConfig {
    line_comments: HASH_LINE_COMMENTS,
    ..LanguageInputConfig::empty()
};
const PYTHON_INPUT: LanguageInputConfig = LanguageInputConfig {
    line_comments: HASH_LINE_COMMENTS,
    block_comment: Some(PYTHON_BLOCK_COMMENT),
    ..LanguageInputConfig::empty()
};
const C_INPUT: LanguageInputConfig = LanguageInputConfig {
    line_comments: C_LINE_COMMENTS,
    block_comment: Some(C_BLOCK_COMMENT),
    documentation_comment: Some(C_DOC_COMMENT),
    ..LanguageInputConfig::empty()
};
const CPP_INPUT: LanguageInputConfig = LanguageInputConfig {
    line_comments: CPP_LINE_COMMENTS,
    block_comment: Some(C_BLOCK_COMMENT),
    documentation_comment: Some(C_DOC_COMMENT),
    ..LanguageInputConfig::empty()
};
const GO_INPUT: LanguageInputConfig = LanguageInputConfig {
    line_comments: SCRIPT_LINE_COMMENTS,
    block_comment: Some(C_BLOCK_COMMENT),
    documentation_comment: Some(C_DOC_COMMENT),
    ..LanguageInputConfig::empty()
};
const CSS_INPUT: LanguageInputConfig = LanguageInputConfig {
    block_comment: Some(SCRIPT_BLOCK_COMMENT),
    ..LanguageInputConfig::empty()
};

const RUST_PAIRS: &[AutoClosePair] = &[
    AutoClosePair {
        start: "{",
        end: "}",
        close: true,
        surround: true,
        newline: true,
        not_in: &[],
    },
    AutoClosePair {
        start: "r###\"",
        end: "\"###",
        close: true,
        surround: true,
        newline: true,
        not_in: &["string", "comment"],
    },
    AutoClosePair {
        start: "r##\"",
        end: "\"##",
        close: true,
        surround: true,
        newline: true,
        not_in: &["string", "comment"],
    },
    AutoClosePair {
        start: "r#\"",
        end: "\"#",
        close: true,
        surround: true,
        newline: true,
        not_in: &["string", "comment"],
    },
    AutoClosePair {
        start: "[",
        end: "]",
        close: true,
        surround: true,
        newline: true,
        not_in: &[],
    },
    AutoClosePair {
        start: "(",
        end: ")",
        close: true,
        surround: true,
        newline: true,
        not_in: &[],
    },
    AutoClosePair {
        start: "<",
        end: ">",
        close: false,
        surround: true,
        newline: true,
        not_in: &["string", "comment"],
    },
    AutoClosePair {
        start: "\"",
        end: "\"",
        close: true,
        surround: true,
        newline: false,
        not_in: &["string"],
    },
    AutoClosePair {
        start: "`",
        end: "`",
        close: false,
        surround: true,
        newline: false,
        not_in: &["string"],
    },
    AutoClosePair {
        start: "/*",
        end: " */",
        close: true,
        surround: true,
        newline: false,
        not_in: &["string", "comment"],
    },
];

const SCRIPT_PAIRS: &[AutoClosePair] = &[
    AutoClosePair {
        start: "{",
        end: "}",
        close: true,
        surround: true,
        newline: true,
        not_in: &[],
    },
    AutoClosePair {
        start: "[",
        end: "]",
        close: true,
        surround: true,
        newline: true,
        not_in: &[],
    },
    AutoClosePair {
        start: "(",
        end: ")",
        close: true,
        surround: true,
        newline: true,
        not_in: &[],
    },
    AutoClosePair {
        start: "<",
        end: ">",
        close: false,
        surround: true,
        newline: true,
        not_in: &["string", "comment"],
    },
    AutoClosePair {
        start: "\"",
        end: "\"",
        close: true,
        surround: true,
        newline: false,
        not_in: &["string", "comment"],
    },
    AutoClosePair {
        start: "'",
        end: "'",
        close: true,
        surround: true,
        newline: false,
        not_in: &["string", "comment"],
    },
    AutoClosePair {
        start: "`",
        end: "`",
        close: true,
        surround: true,
        newline: false,
        not_in: &["string", "comment"],
    },
    AutoClosePair {
        start: "/*",
        end: " */",
        close: true,
        surround: true,
        newline: false,
        not_in: &["string", "comment"],
    },
];

const TS_PAIRS: &[AutoClosePair] = &[
    AutoClosePair {
        start: "{",
        end: "}",
        close: true,
        surround: true,
        newline: true,
        not_in: &[],
    },
    AutoClosePair {
        start: "[",
        end: "]",
        close: true,
        surround: true,
        newline: true,
        not_in: &[],
    },
    AutoClosePair {
        start: "(",
        end: ")",
        close: true,
        surround: true,
        newline: true,
        not_in: &[],
    },
    AutoClosePair {
        start: "<",
        end: ">",
        close: false,
        surround: true,
        newline: true,
        not_in: &["string", "comment"],
    },
    AutoClosePair {
        start: "\"",
        end: "\"",
        close: true,
        surround: true,
        newline: false,
        not_in: &["string"],
    },
    AutoClosePair {
        start: "'",
        end: "'",
        close: true,
        surround: true,
        newline: false,
        not_in: &["string", "comment"],
    },
    AutoClosePair {
        start: "`",
        end: "`",
        close: true,
        surround: true,
        newline: false,
        not_in: &["string"],
    },
    AutoClosePair {
        start: "/*",
        end: " */",
        close: true,
        surround: true,
        newline: false,
        not_in: &["string", "comment"],
    },
];

const MARKDOWN_PAIRS: &[AutoClosePair] = &[
    AutoClosePair {
        start: "{",
        end: "}",
        close: true,
        surround: true,
        newline: true,
        not_in: &[],
    },
    AutoClosePair {
        start: "[",
        end: "]",
        close: true,
        surround: true,
        newline: true,
        not_in: &[],
    },
    AutoClosePair {
        start: "(",
        end: ")",
        close: true,
        surround: true,
        newline: true,
        not_in: &[],
    },
    AutoClosePair {
        start: "<",
        end: ">",
        close: true,
        surround: true,
        newline: true,
        not_in: &[],
    },
    AutoClosePair {
        start: "\"",
        end: "\"",
        close: false,
        surround: true,
        newline: false,
        not_in: &[],
    },
    AutoClosePair {
        start: "'",
        end: "'",
        close: false,
        surround: true,
        newline: false,
        not_in: &[],
    },
    AutoClosePair {
        start: "`",
        end: "`",
        close: false,
        surround: true,
        newline: false,
        not_in: &[],
    },
    AutoClosePair {
        start: "*",
        end: "*",
        close: false,
        surround: true,
        newline: false,
        not_in: &[],
    },
    AutoClosePair {
        start: "~",
        end: "~",
        close: false,
        surround: true,
        newline: false,
        not_in: &[],
    },
];

/// 所有内置语言规格。
///
/// 新增语言时在此登记一个完整规格；
/// 每门文件语言在 `queries/<language>/` 独立提供结构查询。
pub(crate) fn builtin_languages() -> Vec<LanguageSpec> {
    vec![
        LanguageSpec::tree_sitter(
            "Rust",
            LanguageMatcher {
                suffixes: &["rs", "rlib"],
                first_line_pattern: None,
            },
            || tree_sitter_rust::LANGUAGE.into(),
            file_language_queries!("rust")
                .with_injections(include_str!("../queries/rust/injections.scm"))
                .with_outline(include_str!("../queries/rust/outline.scm"))
                .with_locals(include_str!("../queries/rust/locals.scm"))
                .with_overrides(include_str!("../queries/rust/overrides.scm")),
            None,
            RUST_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        )
        .with_input(RUST_INPUT),
        LanguageSpec::tree_sitter(
            "C",
            LanguageMatcher {
                suffixes: &["c"],
                first_line_pattern: None,
            },
            || tree_sitter_c::LANGUAGE.into(),
            file_language_queries!("c", tree_sitter_c::HIGHLIGHT_QUERY)
                .with_injections(include_str!("../queries/c/injections.scm"))
                .with_locals(include_str!("../queries/c/locals.scm")),
            None,
            COMMON_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        )
        .with_input(C_INPUT),
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
            file_language_queries!("cpp")
                .with_injections(include_str!("../queries/cpp/injections.scm"))
                .with_locals(include_str!("../queries/cpp/locals.scm")),
            None,
            COMMON_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        )
        .with_input(CPP_INPUT),
        LanguageSpec::tree_sitter(
            "C#",
            LanguageMatcher {
                suffixes: &["cs", "csx"],
                first_line_pattern: None,
            },
            || tree_sitter_c_sharp::LANGUAGE.into(),
            file_language_queries!("c_sharp", tree_sitter_c_sharp::HIGHLIGHTS_QUERY),
            None,
            COMMON_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        ),
        LanguageSpec::tree_sitter(
            "Go",
            LanguageMatcher {
                suffixes: &["go"],
                first_line_pattern: first_line(r"^//.*\bgo run\b"),
            },
            || tree_sitter_go::LANGUAGE.into(),
            file_language_queries!("go")
                .with_injections(include_str!("../queries/go/injections.scm"))
                .with_locals(include_str!("../queries/go/locals.scm")),
            Some("golang"),
            COMMON_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        )
        .with_input(GO_INPUT),
        LanguageSpec::tree_sitter(
            "Python",
            LanguageMatcher {
                suffixes: &["py", "pyw", "pyx"],
                first_line_pattern: first_line(
                    r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?python[\d.]*",
                ),
            },
            || tree_sitter_python::LANGUAGE.into(),
            file_language_queries!("python")
                .with_injections(include_str!("../queries/python/injections.scm"))
                .with_outline(include_str!("../queries/python/outline.scm"))
                .with_locals(include_str!("../queries/python/locals.scm")),
            None,
            COMMON_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        )
        .with_input(PYTHON_INPUT),
        LanguageSpec::tree_sitter(
            "JavaScript",
            LanguageMatcher {
                suffixes: &["js", "mjs", "cjs"],
                first_line_pattern: first_line(
                    r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?(?:node|bun|deno)",
                ),
            },
            || tree_sitter_typescript::LANGUAGE_TSX.into(),
            file_language_queries!("javascript")
                .with_injections(include_str!("../queries/javascript/injections.scm"))
                .with_outline(include_str!("../queries/javascript/outline.scm"))
                .with_locals(include_str!("../queries/javascript/locals.scm"))
                .with_overrides(include_str!("../queries/javascript/overrides.scm")),
            None,
            SCRIPT_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        )
        .with_input(SCRIPT_INPUT)
        .with_word_characters("$#"),
        LanguageSpec::tree_sitter(
            "JSX",
            LanguageMatcher {
                suffixes: &["jsx"],
                first_line_pattern: None,
            },
            || tree_sitter_typescript::LANGUAGE_TSX.into(),
            file_language_queries!("jsx", include_str!("../queries/javascript/highlights.scm"))
                .with_injections(include_str!("../queries/jsx/injections.scm"))
                .with_outline(include_str!("../queries/jsx/outline.scm"))
                .with_locals(include_str!("../queries/jsx/locals.scm"))
                .with_overrides(include_str!("../queries/jsx/overrides.scm")),
            None,
            TS_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        )
        .with_input(JSX_INPUT)
        .with_word_characters("$#"),
        LanguageSpec::tree_sitter(
            "TypeScript",
            LanguageMatcher {
                suffixes: &["ts", "mts", "cts"],
                first_line_pattern: None,
            },
            || tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            file_language_queries!("typescript")
                .with_injections(include_str!("../queries/typescript/injections.scm"))
                .with_outline(include_str!("../queries/typescript/outline.scm"))
                .with_locals(include_str!("../queries/typescript/locals.scm"))
                .with_overrides(include_str!("../queries/typescript/overrides.scm")),
            None,
            TS_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        )
        .with_input(SCRIPT_INPUT)
        .with_word_characters("$#"),
        LanguageSpec::tree_sitter(
            "TSX",
            LanguageMatcher {
                suffixes: &["tsx"],
                first_line_pattern: None,
            },
            || tree_sitter_typescript::LANGUAGE_TSX.into(),
            file_language_queries!("tsx")
                .with_injections(include_str!("../queries/tsx/injections.scm"))
                .with_outline(include_str!("../queries/tsx/outline.scm"))
                .with_locals(include_str!("../queries/tsx/locals.scm"))
                .with_overrides(include_str!("../queries/tsx/overrides.scm")),
            None,
            TS_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        )
        .with_input(JSX_INPUT)
        .with_word_characters("$#"),
        LanguageSpec::tree_sitter(
            "Java",
            LanguageMatcher {
                suffixes: &["java"],
                first_line_pattern: None,
            },
            || tree_sitter_java::LANGUAGE.into(),
            file_language_queries!("java"),
            None,
            COMMON_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        ),
        LanguageSpec::tree_sitter(
            "Kotlin",
            LanguageMatcher {
                suffixes: &["kt", "kts"],
                first_line_pattern: None,
            },
            || tree_sitter_kotlin_ng::LANGUAGE.into(),
            file_language_queries!("kotlin"),
            None,
            COMMON_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
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
            file_language_queries!("bash"),
            None,
            COMMON_PAIRS,
            SHELL_INPUT_FOLLOWERS,
        )
        .with_input(SHELL_INPUT),
        LanguageSpec::tree_sitter(
            "Ruby",
            LanguageMatcher {
                suffixes: &["rb", "rake", "gemspec"],
                first_line_pattern: first_line(
                    r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?ruby(?:\s|$)",
                ),
            },
            || tree_sitter_ruby::LANGUAGE.into(),
            file_language_queries!("ruby", tree_sitter_ruby::HIGHLIGHTS_QUERY),
            None,
            COMMON_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
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
            file_language_queries!("php", tree_sitter_php::HIGHLIGHTS_QUERY)
                .with_injections(include_str!("../queries/php/injections.scm")),
            None,
            COMMON_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
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
            file_language_queries!("swift", tree_sitter_swift::HIGHLIGHTS_QUERY),
            None,
            COMMON_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
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
            file_language_queries!("lua", tree_sitter_lua::HIGHLIGHTS_QUERY)
                .with_injections(include_str!("../queries/lua/injections.scm")),
            None,
            COMMON_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        ),
        LanguageSpec::tree_sitter(
            "Zig",
            LanguageMatcher {
                suffixes: &["zig"],
                first_line_pattern: None,
            },
            || tree_sitter_zig::LANGUAGE.into(),
            file_language_queries!("zig", tree_sitter_zig::HIGHLIGHTS_QUERY),
            None,
            COMMON_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        ),
        LanguageSpec::tree_sitter(
            "SQL",
            LanguageMatcher {
                suffixes: &["sql"],
                first_line_pattern: None,
            },
            || tree_sitter_sequel::LANGUAGE.into(),
            file_language_queries!("sql"),
            None,
            COMMON_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        ),
        LanguageSpec::tree_sitter(
            "TOML",
            LanguageMatcher {
                suffixes: &["toml"],
                first_line_pattern: None,
            },
            || tree_sitter_toml_ng::LANGUAGE.into(),
            file_language_queries!("toml"),
            None,
            COMMON_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        ),
        LanguageSpec::tree_sitter(
            "JSON",
            LanguageMatcher {
                suffixes: &["json"],
                first_line_pattern: None,
            },
            || tree_sitter_json::LANGUAGE.into(),
            file_language_queries!("json"),
            None,
            COMMON_PAIRS,
            JSON_INPUT_FOLLOWERS,
        )
        .with_input(JSON_INPUT)
        .with_word_characters("#"),
        LanguageSpec::tree_sitter(
            "YAML",
            LanguageMatcher {
                suffixes: &["yaml", "yml"],
                first_line_pattern: None,
            },
            || tree_sitter_yaml::LANGUAGE.into(),
            file_language_queries!("yaml")
                .with_injections(include_str!("../queries/yaml/injections.scm")),
            None,
            COMMON_PAIRS,
            JSON_INPUT_FOLLOWERS,
        )
        .with_input(YAML_INPUT),
        LanguageSpec::tree_sitter(
            "Markdown",
            LanguageMatcher {
                suffixes: &["md", "markdown"],
                first_line_pattern: None,
            },
            || tree_sitter_md::LANGUAGE.into(),
            file_language_queries!("markdown")
                .with_injections(include_str!("../queries/markdown/injections.scm"))
                .with_outline(include_str!("../queries/markdown/outline.scm")),
            None,
            MARKDOWN_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        )
        .with_input(MARKDOWN_INPUT),
        LanguageSpec::tree_sitter(
            "Markdown Inline",
            LanguageMatcher {
                suffixes: &[],
                first_line_pattern: None,
            },
            || tree_sitter_md::INLINE_LANGUAGE.into(),
            LanguageQuerySources::embedded(include_str!(
                "../queries/markdown_inline/highlights.scm"
            ))
            .with_injections(include_str!("../queries/markdown_inline/injections.scm")),
            Some("markdown_inline"),
            COMMON_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        ),
        LanguageSpec::tree_sitter(
            "HTML",
            LanguageMatcher {
                suffixes: &["html", "htm", "xhtml"],
                first_line_pattern: None,
            },
            || tree_sitter_html::LANGUAGE.into(),
            file_language_queries!("html")
                .with_injections(include_str!("../queries/html/injections.scm"))
                .with_outline(include_str!("../queries/html/outline.scm")),
            None,
            COMMON_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        ),
        LanguageSpec::tree_sitter(
            "CSS",
            LanguageMatcher {
                suffixes: &["css", "scss", "less", "sass"],
                first_line_pattern: None,
            },
            || tree_sitter_css::LANGUAGE.into(),
            file_language_queries!("css"),
            None,
            COMMON_PAIRS,
            DEFAULT_INPUT_FOLLOWERS,
        )
        .with_input(CSS_INPUT)
        .with_word_characters("#"),
        LanguageSpec::plain_text(
            "纯文本",
            LanguageMatcher {
                suffixes: &["txt", "text"],
                first_line_pattern: None,
            },
        ),
    ]
}
