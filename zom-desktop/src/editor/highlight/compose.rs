//! 高亮装饰 composer。
//!
//! app / text routing 只产出 [`Decoration`] 语义；本模块在 shell 侧把语义
//! 解析为主题颜色，并输出给绘制 phase。

use gpui::Hsla;
use zom_engine::TextRange;

use super::{Decoration, DecorationKind, DecorationStyle, StyleClass};
use crate::theme::{color, syntax};

pub(crate) struct Composition {
    pub(crate) foreground: Vec<(TextRange, Hsla)>,
    pub(crate) background: Vec<(TextRange, Hsla)>,
}

impl Composition {
    pub(crate) fn empty() -> Self {
        Self {
            foreground: Vec::new(),
            background: Vec::new(),
        }
    }
}

pub(crate) fn compose(decorations: Vec<Decoration>) -> Composition {
    if decorations.is_empty() {
        return Composition::empty();
    }
    let mut foreground: Vec<(TextRange, Hsla)> = Vec::new();
    let mut background: Vec<(u16, TextRange, Hsla)> = Vec::new();
    for d in decorations {
        let resolved = resolve(d.style);
        match d.kind {
            DecorationKind::Foreground => foreground.push((d.range, resolved)),
            DecorationKind::Background => background.push((d.priority, d.range, resolved)),
        }
    }
    foreground.sort_by_key(|(range, _)| range.start());
    background.sort_by_key(|(priority, range, _)| (*priority, range.start()));
    Composition {
        foreground,
        background: background
            .into_iter()
            .map(|(_, range, resolved)| (range, resolved))
            .collect(),
    }
}

fn resolve(style: DecorationStyle) -> Hsla {
    match style {
        DecorationStyle::Named(class) => resolve_named(class),
    }
}

fn resolve_named(class: StyleClass) -> Hsla {
    match class {
        StyleClass::SelectionBackground => color::blue::a05().into(),
        StyleClass::SearchNormalBackground => color::blue::a05().into(),
        StyleClass::SearchCurrentBackground => color::yellow::a05().into(),
        StyleClass::Syntax(name) => syntax::color_for(name.as_str()),
    }
}
