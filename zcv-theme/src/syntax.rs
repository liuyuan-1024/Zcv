//! 语法高亮：tree-sitter capture name → GPUI HighlightStyle。
//!
//! 本模块只提供查询机制，不定义色值。色值来自主题 TOML，由 `theme_data` 单一解析器解析后经 `set_theme` 注入。
//! 查询走点分前缀回退：`keyword.control.import` 未命中 → `keyword.control` → `keyword` → 默认样式。

use std::collections::BTreeMap;
use std::ops::Bound;
use std::sync::Arc;

use gpui::{App, Global, HighlightStyle};

use crate::theme_data::ThemeData;
#[cfg(test)]
use crate::theme_data::theme_by_id;

/// 当前语法高亮表的 App 级 global 载体；随主题切换整体替换。
struct SyntaxGlobal(Arc<BTreeMap<&'static str, HighlightStyle>>);

impl Global for SyntaxGlobal {}

/// 预展开 capture 名字表为按索引直接取用的样式表。
///
/// 每个名字做一次点分前缀回退；
/// 渲染侧按 capture index 一次数组索引，不再逐 run 做字符串查找与回退。
pub fn style_table(names: &[Arc<str>], cx: &App) -> Vec<HighlightStyle> {
    let table = active_table(cx);
    names
        .iter()
        .map(|name| style_for_table(table.as_ref(), name))
        .collect()
}

/// 当前 App 的语法表；未注入主题时回退首个内置主题，保持无主题上下文可渲染。
fn active_table(cx: &App) -> Arc<BTreeMap<&'static str, HighlightStyle>> {
    cx.try_global::<SyntaxGlobal>()
        .map(|global| Arc::clone(&global.0))
        .unwrap_or_else(|| Arc::clone(&crate::first_theme().syntax_table))
}

/// 在指定语法表中解析 capture name。
fn style_for_table(theme: &BTreeMap<&'static str, HighlightStyle>, name: &str) -> HighlightStyle {
    // range 覆盖「首段 … 全名」：命中候选都是 name 的前缀，rfind 取最长（最深）的一条。
    let first_segment = name.split('.').next().unwrap_or(name);
    theme
        .range::<str, _>((Bound::Included(first_segment), Bound::Included(name)))
        .rfind(|(prefix, _)| {
            name.strip_prefix(*prefix)
                .is_some_and(|remainder| remainder.is_empty() || remainder.starts_with('.'))
        })
        .map(|(_, style)| *style)
        .unwrap_or_default()
}

/// 注入主题的语法高亮表（主题切换时由 [`ThemeChoice::apply`] 调用）。
pub(crate) fn set_theme(theme: &ThemeData, cx: &mut App) {
    cx.set_global(SyntaxGlobal(Arc::clone(&theme.syntax_table)));
}

#[cfg(test)]
#[path = "test/syntax_tests.rs"]
mod tests;
