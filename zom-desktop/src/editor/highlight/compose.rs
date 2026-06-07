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
    let mut foreground: Vec<(u16, TextRange, Hsla)> = Vec::new();
    let mut background: Vec<(u16, TextRange, Hsla)> = Vec::new();
    for d in decorations {
        let resolved = resolve(d.style);
        match d.kind {
            DecorationKind::Foreground => foreground.push((d.priority, d.range, resolved)),
            DecorationKind::Background => background.push((d.priority, d.range, resolved)),
        }
    }
    let foreground = compose_foreground(foreground);
    background.sort_by_key(|(priority, range, _)| (*priority, range.start()));
    Composition {
        foreground,
        background: background
            .into_iter()
            .map(|(_, range, resolved)| (range, resolved))
            .collect(),
    }
}

fn compose_foreground(mut foreground: Vec<(u16, TextRange, Hsla)>) -> Vec<(TextRange, Hsla)> {
    foreground.sort_by_key(|(priority, range, _)| (*priority, range.start()));

    let mut composed: Vec<(TextRange, Hsla)> = Vec::new();
    for (_, range, color) in foreground {
        let mut next = Vec::with_capacity(composed.len() + 1);
        for (existing, existing_color) in composed {
            for remainder in subtract_range(existing, range) {
                next.push((remainder, existing_color));
            }
        }
        next.push((range, color));
        composed = next;
    }

    composed.sort_by_key(|(range, _)| range.start());
    composed
}

fn subtract_range(existing: TextRange, cover: TextRange) -> Vec<TextRange> {
    if !existing.overlaps(cover) {
        return vec![existing];
    }

    let mut out = Vec::with_capacity(2);
    if existing.start() < cover.start()
        && let Ok(left) = TextRange::new(existing.start(), cover.start())
        && !left.is_empty()
    {
        out.push(left);
    }
    if cover.end() < existing.end()
        && let Ok(right) = TextRange::new(cover.end(), existing.end())
        && !right.is_empty()
    {
        out.push(right);
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;
    use zom_engine::ByteOffset;

    fn range(start: usize, end: usize) -> TextRange {
        TextRange::new(ByteOffset::new(start), ByteOffset::new(end)).unwrap()
    }

    fn syntax_decoration(priority: u16, range: TextRange, name: &str) -> Decoration {
        Decoration {
            range,
            kind: DecorationKind::Foreground,
            style: DecorationStyle::Named(StyleClass::Syntax(name.to_string())),
            priority,
        }
    }

    #[test]
    fn foreground_priority_clips_lower_priority_runs() {
        let composition = compose(vec![
            syntax_decoration(0, range(0, 6), "keyword"),
            syntax_decoration(1, range(2, 4), "string"),
        ]);

        let ranges = composition
            .foreground
            .iter()
            .map(|(range, _)| (range.start().get(), range.end().get()))
            .collect::<Vec<_>>();
        assert_eq!(ranges, vec![(0, 2), (2, 4), (4, 6)]);
    }
}
