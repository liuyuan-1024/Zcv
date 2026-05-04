//! M6 光标、选区与多光标模型。
//!
//! M6 起，`SelectionSet` 是编辑引擎里的主选区模型；不再把 M3 的
//! `SelectionSnapshot` 作为兼容层继续传播。

use crate::{ChangeSet, CharOffset, TextRange};

/// 单个插入光标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Cursor {
    offset: CharOffset,
}

impl Cursor {
    pub const fn new(offset: CharOffset) -> Self {
        Self { offset }
    }

    pub const fn offset(self) -> CharOffset {
        self.offset
    }

    pub const fn to_selection(self) -> Selection {
        Selection::caret(self.offset)
    }
}

/// 光标 / Anchor 的吸附方向。
///
/// 当前 M6 先保留类型，完整 Anchor / TrackedRange stickiness 后续 M7 再接入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Affinity {
    /// 同点插入时偏向插入内容之前。
    Before,
    /// 同点插入时偏向插入内容之后。
    #[default]
    After,
}

/// M6B 文本移动方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MovementDirection {
    /// 向前 / 向左移动。
    Previous,
    /// 向后 / 向右移动。
    Next,
}

/// M6B 文本移动粒度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MovementUnit {
    /// 用户感知字符，复用 M5A grapheme boundary。
    Grapheme,
    /// Unicode 自然语言 word，基于 `unicode-segmentation`。
    Word,
    /// 编程语言标识符片段，默认包含 Unicode 字母数字、组合音标、`_` 和 `$`。
    Identifier,
    /// 标识符内的子词，支持 snake_case、camelCase、PascalCase 与字母/数字切分。
    Subword,
    /// 操作符 / 标点 / emoji 等非空白、非 identifier 的符号 run。
    Symbol,
}

/// M6C 组合输入中的相对选区。
///
/// `anchor` / `head` 是相对于当前 preedit 文本开头的 `CharOffset`，
/// 不是整个 Buffer 的绝对坐标。`Buffer::update_composition` 会把它映射为
/// 当前文档中的绝对 `Selection`，并验证它没有落在 grapheme 中间。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CompositionSelection {
    anchor: CharOffset,
    head: CharOffset,
}

impl CompositionSelection {
    pub const fn new(anchor: CharOffset, head: CharOffset) -> Self {
        Self { anchor, head }
    }

    pub const fn caret(offset: CharOffset) -> Self {
        Self {
            anchor: offset,
            head: offset,
        }
    }

    pub const fn anchor(self) -> CharOffset {
        self.anchor
    }

    pub const fn head(self) -> CharOffset {
        self.head
    }
}

/// M6C 当前组合输入状态。
///
/// `range` 是当前 preedit 文本在 Buffer 中的绝对范围；`selection` 是当前组合态
/// 选区在 Buffer 中的绝对位置。`original_*` 字段用于 cancel / commit 时恢复
/// 或生成合理的单步 Undo 历史。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositionState {
    pub(crate) original_text: String,
    pub(crate) original_selection: SelectionSet,
    pub(crate) original_was_dirty: bool,
    pub(crate) range: TextRange,
    pub(crate) preedit_text: String,
    pub(crate) selection: Selection,
}

impl CompositionState {
    pub(crate) fn new(
        original_text: String,
        original_selection: SelectionSet,
        original_was_dirty: bool,
        range: TextRange,
    ) -> Self {
        Self {
            original_text,
            original_selection,
            original_was_dirty,
            range,
            preedit_text: String::new(),
            selection: Selection::caret(range.start()),
        }
    }

    pub fn range(&self) -> TextRange {
        self.range
    }

    pub fn preedit_text(&self) -> &str {
        &self.preedit_text
    }

    pub fn selection(&self) -> Selection {
        self.selection
    }

    pub fn original_selection(&self) -> &SelectionSet {
        &self.original_selection
    }

    pub fn original_was_dirty(&self) -> bool {
        self.original_was_dirty
    }
}

/// 一个选区，使用 anchor/head 模型。
///
/// `anchor` 是固定端，`head` 是活动端。两者相等时表示 caret。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Selection {
    anchor: CharOffset,
    head: CharOffset,
}

impl Selection {
    pub const fn new(anchor: CharOffset, head: CharOffset) -> Self {
        Self { anchor, head }
    }

    pub const fn caret(offset: CharOffset) -> Self {
        Self {
            anchor: offset,
            head: offset,
        }
    }

    pub const fn anchor(self) -> CharOffset {
        self.anchor
    }

    pub const fn head(self) -> CharOffset {
        self.head
    }

    pub fn cursor(self) -> Option<Cursor> {
        if self.is_caret() {
            Some(Cursor::new(self.head))
        } else {
            None
        }
    }

    pub fn is_caret(self) -> bool {
        self.anchor == self.head
    }

    pub fn is_reversed(self) -> bool {
        self.anchor > self.head
    }

    pub fn start(self) -> CharOffset {
        self.anchor.min(self.head)
    }

    pub fn end(self) -> CharOffset {
        self.anchor.max(self.head)
    }

    pub fn range(self) -> TextRange {
        TextRange::new(self.start(), self.end())
            .expect("Selection start/end 由 min/max 生成，必须满足 start <= end")
    }

    pub fn collapse_to_start(self) -> Self {
        Self::caret(self.start())
    }

    pub fn collapse_to_end(self) -> Self {
        Self::caret(self.end())
    }

    pub fn with_head(self, head: CharOffset) -> Self {
        Self {
            anchor: self.anchor,
            head,
        }
    }

    pub fn map_through_changeset(self, changeset: &ChangeSet) -> Self {
        Self {
            anchor: changeset.map_position(self.anchor),
            head: changeset.map_position(self.head),
        }
    }
}

/// 选区归一化时对相邻区间的处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SelectionMergePolicy {
    /// 只合并重叠区间、重复 caret、以及落在选区边界上的 caret。
    #[default]
    MergeOverlapping,
    /// 额外合并首尾相接的非空区间。
    MergeOverlappingOrAdjacent,
}

/// 归一化后的多选区 / 多光标集合。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SelectionSet {
    selections: Vec<Selection>,
    primary_index: usize,
}

impl SelectionSet {
    /// 创建选区集合。空集合会被规范化为文首单 caret。
    pub fn new(selections: Vec<Selection>) -> Self {
        Self::new_with_primary(selections, 0)
    }

    pub fn new_with_primary(selections: Vec<Selection>, primary_index: usize) -> Self {
        Self::new_with_policy(
            selections,
            primary_index,
            SelectionMergePolicy::MergeOverlapping,
        )
    }

    pub fn new_with_policy(
        selections: Vec<Selection>,
        primary_index: usize,
        policy: SelectionMergePolicy,
    ) -> Self {
        normalize_selections(selections, primary_index, policy)
    }

    pub fn caret(offset: CharOffset) -> Self {
        Self {
            selections: vec![Selection::caret(offset)],
            primary_index: 0,
        }
    }

    pub fn from_ranges(ranges: Vec<TextRange>) -> Self {
        Self::new(
            ranges
                .into_iter()
                .map(|range| Selection::new(range.start(), range.end()))
                .collect(),
        )
    }

    pub fn as_slice(&self) -> &[Selection] {
        &self.selections
    }

    pub fn len(&self) -> usize {
        self.selections.len()
    }

    pub fn is_empty(&self) -> bool {
        self.selections.is_empty()
    }

    pub fn primary_index(&self) -> usize {
        self.primary_index
    }

    pub fn primary(&self) -> &Selection {
        &self.selections[self.primary_index]
    }

    pub fn ranges(&self) -> Vec<TextRange> {
        self.selections
            .iter()
            .copied()
            .map(Selection::range)
            .collect()
    }

    pub fn normalized(&self) -> Self {
        Self::new_with_primary(self.selections.clone(), self.primary_index)
    }

    pub fn map_through_changeset(&self, changeset: &ChangeSet) -> Self {
        Self::new_with_primary(
            self.selections
                .iter()
                .copied()
                .map(|selection| selection.map_through_changeset(changeset))
                .collect(),
            self.primary_index,
        )
    }

    pub fn into_vec(self) -> Vec<Selection> {
        self.selections
    }
}

impl Default for SelectionSet {
    fn default() -> Self {
        Self::caret(CharOffset::ZERO)
    }
}

fn normalize_selections(
    selections: Vec<Selection>,
    primary_index: usize,
    policy: SelectionMergePolicy,
) -> SelectionSet {
    if selections.is_empty() {
        return SelectionSet::caret(CharOffset::ZERO);
    }

    let original_primary_index = primary_index.min(selections.len() - 1);
    let original_primary_head = selections[original_primary_index].head();

    let mut indexed: Vec<(usize, Selection)> = selections.into_iter().enumerate().collect();
    indexed.sort_by_key(|(_, selection)| {
        (
            selection.start(),
            selection.end(),
            selection.head(),
            selection.anchor(),
        )
    });

    let mut merged: Vec<Selection> = Vec::new();

    for (_, selection) in indexed {
        let Some(current) = merged.last_mut() else {
            merged.push(selection);
            continue;
        };

        if should_merge(*current, selection, policy) {
            let start = current.start().min(selection.start());
            let end = current.end().max(selection.end());
            *current = Selection::new(start, end);
        } else {
            merged.push(selection);
        }
    }

    let primary_index = merged
        .iter()
        .position(|selection| contains_offset(*selection, original_primary_head))
        .unwrap_or_else(|| {
            merged
                .iter()
                .position(|selection| selection.head() >= original_primary_head)
                .unwrap_or(merged.len() - 1)
        });

    SelectionSet {
        selections: merged,
        primary_index,
    }
}

fn should_merge(current: Selection, next: Selection, policy: SelectionMergePolicy) -> bool {
    if current.end() > next.start() {
        return true;
    }

    if current.end() == next.start() {
        return match policy {
            SelectionMergePolicy::MergeOverlappingOrAdjacent => true,
            SelectionMergePolicy::MergeOverlapping => current.is_caret() || next.is_caret(),
        };
    }

    false
}

fn contains_offset(selection: Selection, offset: CharOffset) -> bool {
    selection.start() <= offset && offset <= selection.end()
}
