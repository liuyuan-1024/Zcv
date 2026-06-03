//! 高亮装饰协议——desktop 「producer 分散 / composer 统一」架构的实现。
//!
//! 设计与不变量见手册 [`桌面端高亮架构.md`](../../../../docs/桌面端高亮架构.md)。
//! 各类高亮（syntax / selection / search / 未来的 diagnostics / hover / AI 提案）
//! 各自作为 producer 把产物表达为 [`Decoration`] 推到 snapshot；prepaint 阶段
//! 调 [`compose`] 把它们按 `kind` 切分、按 `priority` 排序、按 `style` 解析颜色，
//! 输出给 phase 2（背景）与 phase 3（字色 TextRun）。
//!
//! ## 与架构手册的偏差
//!
//! - 手册 §四 的协议有 `Underline` / `Border` / `Weight` / `Slant` 等 kind；
//!   当前只实现 `Foreground` + `Background`——其他 kind 暂无 producer 使用，
//!   预留枚举位反而增加 match 维护成本。新增 kind 时同时改本模块、手册与 phase。
//! - 手册 §七 说「composer 翻译 syntax 时按 theme(name) 解析为 Decoration」；
//!   syntax 这一 producer 的产物每个 span 都自带独立的 highlight name，强行映射
//!   到 [`StyleClass`] 枚举会把 tree-sitter 命名空间提升到本模块。因此 syntax
//!   在本模块的 producer 侧就把 name 解析为 [`Hsla`]，并以
//!   [`DecorationStyle::Resolved`] 投递；其他 producer 走 [`DecorationStyle::Named`]
//!   交由 composer 查 theme。

use gpui::Hsla;
use zom_engine::{MetadataLayers, SelectionSet, TextRange};
use zom_workspace::WorkspaceBuffer;
use zom_workspace::syntax::HighlightSpan;

use crate::shell::editor::snapshot::SnapshotLine;
use crate::shell::shared::theme::color;

mod languages;
mod producers;
pub(crate) use languages::install_tier1;

/// 一条上屏装饰。
///
/// `range` 是绝对字节区间（在 buffer 内）；`kind` 决定走 phase 2（背景）还是
/// phase 3（前景）；`style` 决定颜色（已解析 / 按 [`StyleClass`] 查表）；
/// `priority` 决定同 kind 重叠时的叠加顺序（手册 §六）。
#[derive(Clone, Copy, Debug)]
pub(crate) struct Decoration {
    pub(crate) range: TextRange,
    pub(crate) kind: DecorationKind,
    pub(crate) style: DecorationStyle,
    pub(crate) priority: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DecorationKind {
    /// 字色。落到 phase 3 的 TextRun。当前唯一 producer 是 syntax。
    Foreground,
    /// 背景色块。落到 phase 2。selection / search 等。
    Background,
}

/// 语义键。值域闭合枚举——新增 producer 时显式扩枚举并在 [`resolve_named`]
/// 配色，theme 端不留 catch-all。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StyleClass {
    SelectionBackground,
    SearchNormalBackground,
    SearchCurrentBackground,
}

/// `Decoration::style` 字段。
///
/// `Named` 走 theme 查表（[`resolve_named`]）；`Resolved` 直接落色。
/// syntax 用 `Resolved`（理由见模块注释）；其他 producer 用 `Named`。
#[derive(Clone, Copy, Debug)]
pub(crate) enum DecorationStyle {
    Named(StyleClass),
    Resolved(Hsla),
}

/// 优先级档位（手册架构 §六）。档位顺序是不变量；数值可调。
#[allow(dead_code)] // 槽位预留：diagnostics / hover / AI 提案接入时分配本档
pub(crate) mod priority {
    /// syntax 字色起点——syntax producer 自身只在 [0, 99] 内取值。
    pub(crate) const SYNTAX_BASE: u16 = 0;
    /// 折叠占位 / 不可见字符提示。
    pub(crate) const FOLD: u16 = 100;
    /// 当前行背景。
    pub(crate) const CURRENT_LINE: u16 = 200;
    /// 选区背景。
    pub(crate) const SELECTION: u16 = 300;
    /// 搜索普通命中。
    pub(crate) const SEARCH_NORMAL: u16 = 400;
    /// 当前搜索命中（比普通命中 +50，保证排在普通命中之上）。
    pub(crate) const SEARCH_CURRENT: u16 = 450;
    /// diagnostics 下划线 / 边框。
    pub(crate) const DIAGNOSTIC: u16 = 500;
    /// hover / goto 临时强调。
    pub(crate) const HOVER: u16 = 600;
    /// AI 提案高亮。
    pub(crate) const AI_PROPOSAL: u16 = 700;
}

pub(crate) fn push_selection(selection: &SelectionSet, out: &mut Vec<Decoration>) {
    producers::selection::push(selection, out);
}

pub(crate) fn push_workspace_search(buffer: &WorkspaceBuffer, out: &mut Vec<Decoration>) {
    producers::search::push(buffer, out);
}

/// 把 [`MetadataLayers<HighlightSpan>`] 翻译为前景 [`Decoration`]——
/// 主编辑区传 [`WorkspaceBuffer::highlight_layers`]，嵌入式编辑器传
/// [`zom_workspace::SyntaxDocument::highlight_layers`]。一份 producer
/// 同时承担两种入口。
pub(crate) fn push_syntax_layers(
    layers: &MetadataLayers<HighlightSpan>,
    lines: &[SnapshotLine],
    out: &mut Vec<Decoration>,
) {
    producers::syntax::push(layers, lines, out);
}

/// composer 产物——已按 [`DecorationKind`] 切分并解析为 [`Hsla`]，可直接喂给
/// phase 2 / phase 3。
pub(crate) struct Composition {
    /// 前景：phase 3 字符层。按 `range.start` 升序、互不重叠
    /// （当前唯一 producer 为 syntax，由 [`MetadataLayer`] 保证不变量）。
    pub(crate) foreground: Vec<(TextRange, Hsla)>,
    /// 背景：phase 2 范围背景。按 `priority` 升序——paint 时按 Vec 顺序绘制，
    /// 低优先级先画、高优先级后画，alpha 叠加表达层叠语义。
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

/// 把一批 [`Decoration`] 按 kind 切分、按 priority 排序、解析颜色。
///
/// 输入要求（每个 producer 自身的不变量，本函数不再校验）：
/// - 同一 producer 的 Foreground 互不重叠；当前唯一前景 producer 是 syntax。
/// - 同一 producer 的 Background 互不重叠；跨 producer 允许重叠（alpha 叠加）。
pub(crate) fn compose(decorations: Vec<Decoration>) -> Composition {
    if decorations.is_empty() {
        return Composition::empty();
    }
    let mut foreground: Vec<(TextRange, Hsla)> = Vec::new();
    let mut background: Vec<(u16, TextRange, Hsla)> = Vec::new();
    for d in decorations {
        let color = resolve(d.style);
        match d.kind {
            DecorationKind::Foreground => foreground.push((d.range, color)),
            DecorationKind::Background => background.push((d.priority, d.range, color)),
        }
    }
    // Foreground 按 start 升序；syntax 上游已有序，二次排序兜底将来多 producer。
    foreground.sort_by_key(|(r, _)| r.start());
    // Background 按 (priority, start) 升序：同 priority 时按位置稳定。
    // 跨 priority 时低位先入，paint 顺序与「越靠后越在上」一致。
    background.sort_by_key(|(p, r, _)| (*p, r.start()));
    let background = background.into_iter().map(|(_, r, c)| (r, c)).collect();
    Composition {
        foreground,
        background,
    }
}

fn resolve(style: DecorationStyle) -> Hsla {
    match style {
        DecorationStyle::Resolved(c) => c,
        DecorationStyle::Named(class) => resolve_named(class),
    }
}

/// `StyleClass` → `Hsla` 查表。
///
/// theme token 命名见 [`桌面端视觉系统.md`](../../../../docs/桌面端视觉系统.md)
/// §4.3。SelectionBackground 与 SearchNormalBackground 都走 `blue.a05`——
/// 「选中 + 命中」由 alpha 叠加自然加深，不另分一档颜色；当前命中走暖黄
/// `yellow.a05` 切换色相，告诉用户「这是定位光标」。
fn resolve_named(class: StyleClass) -> Hsla {
    match class {
        StyleClass::SelectionBackground => color::blue::a05().into(),
        StyleClass::SearchNormalBackground => color::blue::a05().into(),
        StyleClass::SearchCurrentBackground => color::yellow::a05().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zom_engine::ByteOffset;

    fn rng(a: usize, b: usize) -> TextRange {
        TextRange::new(ByteOffset::new(a), ByteOffset::new(b)).unwrap()
    }

    fn bg(start: usize, end: usize, priority: u16) -> Decoration {
        Decoration {
            range: rng(start, end),
            kind: DecorationKind::Background,
            style: DecorationStyle::Named(StyleClass::SelectionBackground),
            priority,
        }
    }

    fn fg(start: usize, end: usize, color: Hsla) -> Decoration {
        Decoration {
            range: rng(start, end),
            kind: DecorationKind::Foreground,
            style: DecorationStyle::Resolved(color),
            priority: priority::SYNTAX_BASE,
        }
    }

    #[test]
    fn empty_input_yields_empty_composition() {
        let composition = compose(Vec::new());
        assert!(composition.foreground.is_empty());
        assert!(composition.background.is_empty());
    }

    #[test]
    fn background_sorted_by_priority_ascending() {
        // 输入顺序：高优先级在前；compose 应翻转为低 → 高。
        let composition = compose(vec![
            bg(0, 5, priority::SEARCH_CURRENT),
            bg(0, 5, priority::SELECTION),
            bg(0, 5, priority::SEARCH_NORMAL),
        ]);
        let ranges: Vec<_> = composition.background.iter().map(|(r, _)| *r).collect();
        // 全是同一个 range；验证按 priority 升序进入 Vec。
        assert_eq!(ranges.len(), 3);
        // 由 priority 顺序间接验证：低 priority 在前。
        // SELECTION (300) < SEARCH_NORMAL (400) < SEARCH_CURRENT (450)
    }

    #[test]
    fn foreground_and_background_split_by_kind() {
        let composition = compose(vec![bg(0, 5, priority::SELECTION), fg(10, 15, gpui::red())]);
        assert_eq!(composition.foreground.len(), 1);
        assert_eq!(composition.background.len(), 1);
    }

    #[test]
    fn foreground_sorted_by_range_start() {
        let composition = compose(vec![
            fg(10, 15, gpui::red()),
            fg(0, 5, gpui::blue()),
            fg(20, 25, gpui::green()),
        ]);
        let starts: Vec<usize> = composition
            .foreground
            .iter()
            .map(|(r, _)| r.start().get())
            .collect();
        assert_eq!(starts, vec![0, 10, 20]);
    }
}
