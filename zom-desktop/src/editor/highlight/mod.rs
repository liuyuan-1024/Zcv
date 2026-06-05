//! 编辑器装饰协议与 producer。
//!
//! 本模块只描述“哪些字节范围要以什么语义装饰”，不解析主题颜色，也不绘制。

use zom_engine::{MetadataLayers, SelectionSet, TextRange};
use zom_workspace::WorkspaceBuffer;
use zom_workspace::syntax::HighlightSpan;

use crate::editor::text::snapshot::SnapshotLine;

pub(crate) mod compose;
mod producers;

#[derive(Clone, Debug)]
pub(crate) struct Decoration {
    pub(crate) range: TextRange,
    pub(crate) kind: DecorationKind,
    pub(crate) style: DecorationStyle,
    pub(crate) priority: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DecorationKind {
    Foreground,
    Background,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StyleClass {
    SelectionBackground,
    SearchNormalBackground,
    SearchCurrentBackground,
    Syntax(String),
}

#[derive(Clone, Debug)]
pub(crate) enum DecorationStyle {
    Named(StyleClass),
}

#[allow(dead_code)]
pub(crate) mod priority {
    pub(crate) const SYNTAX_CONFIRMED: u16 = 0;
    pub(crate) const SYNTAX_PROVISIONAL: u16 = 1;
    pub(crate) const FOLD: u16 = 100;
    pub(crate) const CURRENT_LINE: u16 = 200;
    pub(crate) const SELECTION: u16 = 300;
    pub(crate) const SEARCH_NORMAL: u16 = 400;
    pub(crate) const SEARCH_CURRENT: u16 = 450;
    pub(crate) const DIAGNOSTIC: u16 = 500;
    pub(crate) const HOVER: u16 = 600;
    pub(crate) const AI_PROPOSAL: u16 = 700;
}

pub(crate) fn push_selection(selection: &SelectionSet, out: &mut Vec<Decoration>) {
    producers::selection::push(selection, out);
}

pub(crate) fn push_workspace_search(buffer: &WorkspaceBuffer, out: &mut Vec<Decoration>) {
    producers::search::push(buffer, out);
}

pub(crate) fn push_syntax_layers(
    layers: &MetadataLayers<HighlightSpan>,
    lines: &[SnapshotLine],
    out: &mut Vec<Decoration>,
) {
    producers::syntax::push(layers, lines, out);
}
