//! 语言检测：根据文件路径和首行内容推断编程语言。
//!
//! 与 Zed `available_languages.rs` 结构对齐：
//! 每个语言注册 `LanguageMatcher`（路径后缀 + 首行正则），匹配时按"最长后缀胜出"策略择优。

use std::path::Path;

/// 语言匹配规则。
pub struct LanguageMatcher {
    /// 文件后缀列表（如 `["rs", "rlib"]`），匹配时比文件名或扩展名末尾。
    pub path_suffixes: &'static [&'static str],
    /// 首行正则（用于检测 shebang 或 Vim/Emacs modeline）。
    pub first_line_pattern: Option<&'static str>,
}

/// 注册的语言条目。
pub struct LanguageEntry {
    /// 显示名称（如 `"Rust"`）。
    pub name: &'static str,
    /// 匹配规则。
    pub matcher: LanguageMatcher,
}

/// 内置语言注册表。
///
/// 新增语言时只需在此追加一个条目，无需改动匹配逻辑。
static LANGUAGES: &[LanguageEntry] = &[
    LanguageEntry {
        name: "Rust",
        matcher: LanguageMatcher {
            path_suffixes: &["rs", "rlib"],
            first_line_pattern: None,
        },
    },
    LanguageEntry {
        name: "Python",
        matcher: LanguageMatcher {
            path_suffixes: &["py", "pyw", "pyx"],
            first_line_pattern: Some(
                r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?python[\d.]*",
            ),
        },
    },
    LanguageEntry {
        name: "JavaScript",
        matcher: LanguageMatcher {
            path_suffixes: &["js", "mjs", "cjs"],
            first_line_pattern: Some(
                r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?(?:node|bun|deno)",
            ),
        },
    },
    LanguageEntry {
        name: "TypeScript",
        matcher: LanguageMatcher {
            path_suffixes: &["ts", "tsx", "mts", "cts"],
            first_line_pattern: None,
        },
    },
    LanguageEntry {
        name: "JSX",
        matcher: LanguageMatcher {
            path_suffixes: &["jsx"],
            first_line_pattern: None,
        },
    },
    LanguageEntry {
        name: "Go",
        matcher: LanguageMatcher {
            path_suffixes: &["go"],
            first_line_pattern: None,
        },
    },
    LanguageEntry {
        name: "Java",
        matcher: LanguageMatcher {
            path_suffixes: &["java", "class", "jar"],
            first_line_pattern: None,
        },
    },
    LanguageEntry {
        name: "Ruby",
        matcher: LanguageMatcher {
            path_suffixes: &["rb", "erb"],
            first_line_pattern: Some(r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?ruby"),
        },
    },
    LanguageEntry {
        name: "C",
        matcher: LanguageMatcher {
            path_suffixes: &["c", "h"],
            first_line_pattern: None,
        },
    },
    LanguageEntry {
        name: "C++",
        matcher: LanguageMatcher {
            path_suffixes: &["cpp", "cc", "cxx", "c++", "hpp", "hh", "hxx"],
            first_line_pattern: None,
        },
    },
    LanguageEntry {
        name: "Zig",
        matcher: LanguageMatcher {
            path_suffixes: &["zig"],
            first_line_pattern: None,
        },
    },
    LanguageEntry {
        name: "Shell",
        matcher: LanguageMatcher {
            path_suffixes: &["sh", "bash", "zsh", "ksh"],
            first_line_pattern: Some(
                r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?(?:bash|sh|zsh|ksh|dash)",
            ),
        },
    },
    LanguageEntry {
        name: "Lua",
        matcher: LanguageMatcher {
            path_suffixes: &["lua"],
            first_line_pattern: Some(r"^#!\s*(?:/usr/bin/|/bin/|/usr/local/bin/)?(?:env\s+)?lua"),
        },
    },
    LanguageEntry {
        name: "TOML",
        matcher: LanguageMatcher {
            path_suffixes: &["toml"],
            first_line_pattern: None,
        },
    },
    LanguageEntry {
        name: "JSON",
        matcher: LanguageMatcher {
            path_suffixes: &["json"],
            first_line_pattern: None,
        },
    },
    LanguageEntry {
        name: "YAML",
        matcher: LanguageMatcher {
            path_suffixes: &["yaml", "yml"],
            first_line_pattern: None,
        },
    },
    LanguageEntry {
        name: "Markdown",
        matcher: LanguageMatcher {
            path_suffixes: &["md", "markdown"],
            first_line_pattern: None,
        },
    },
    LanguageEntry {
        name: "HTML",
        matcher: LanguageMatcher {
            path_suffixes: &["html", "htm", "xhtml"],
            first_line_pattern: None,
        },
    },
    LanguageEntry {
        name: "CSS",
        matcher: LanguageMatcher {
            path_suffixes: &["css", "scss", "less", "sass"],
            first_line_pattern: None,
        },
    },
    LanguageEntry {
        name: "SQL",
        matcher: LanguageMatcher {
            path_suffixes: &["sql"],
            first_line_pattern: None,
        },
    },
    LanguageEntry {
        name: "LaTeX",
        matcher: LanguageMatcher {
            path_suffixes: &["tex", "latex", "sty", "cls", "bib"],
            first_line_pattern: None,
        },
    },
    LanguageEntry {
        name: "XML",
        matcher: LanguageMatcher {
            path_suffixes: &["xml", "xsd", "xsl", "plist", "svg"],
            first_line_pattern: None,
        },
    },
];

/// 根据文件路径和可选的首次返回语言名称。
///
/// 匹配策略（与 Zed 一致）：
/// 1. 先按路径**后缀**（文件名或扩展名末尾）匹配，**最长后缀胜出**。
/// 2. 路径未命中时，按**首行正则**匹配（shebang / modeline）。
/// 3. 都未命中返回 `None`。
pub fn language_for_file(path: &Path, first_line: Option<&str>) -> Option<&'static str> {
    let filename = path.file_name().and_then(|n| n.to_str())?;
    let extension = filename.split('.').next_back();

    // ── 第 1 轮：路径后缀匹配，选最长 ──
    let mut best: Option<(&'static str, usize)> = None;
    for entry in LANGUAGES {
        for suffix in entry.matcher.path_suffixes {
            let dot_suffix = [".", suffix].concat();
            // 检查文件名是否以 ".suffix" 结尾
            if filename.ends_with(&dot_suffix) || extension == Some(suffix) {
                let len = suffix.len();
                if best.is_none_or(|(_, best_len)| len > best_len) {
                    best = Some((entry.name, len));
                }
            }
        }
    }
    if let Some((name, _)) = best {
        return Some(name);
    }

    // ── 第 2 轮：首行正则匹配 ──
    if let Some(line) = first_line {
        for entry in LANGUAGES {
            if let Some(pattern) = entry.matcher.first_line_pattern
                && let Ok(re) = regex::Regex::new(pattern)
                && re.is_match(line)
            {
                return Some(entry.name);
            }
        }
    }

    None
}
