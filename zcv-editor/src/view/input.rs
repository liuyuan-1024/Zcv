//! 文本输入：IME 组合会话、输入事务与 utf16 坐标换算。
//!
//! 输入法组合的 marked text 从第一次 preedit 起就走普通文本事务（与键盘输入同管线），组合区域只在语法样式之上叠加下划线；
//! 这里只维护组合会话身份与候选框定位数据。

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    App, Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window, px, size,
};
use zcv_engine::{
    ByteOffset, Selection, SelectionSet, Snapshot, TextRange, TransactionId, Utf16Offset,
};

use super::*;
use crate::selection::{EditorSelections, replace_selections};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorComposition {
    pub(super) ranges: Arc<[TextRange]>,
    pub(super) primary_index: usize,
    pub(super) history_transaction_id: Option<TransactionId>,
}

#[derive(Debug, Clone)]
pub(crate) struct EditorPresentation {
    snapshot: Snapshot,
    composition: Option<EditorComposition>,
}

impl EditorPresentation {
    pub(crate) fn new(snapshot: &Snapshot, composition: Option<&EditorComposition>) -> Self {
        Self {
            snapshot: snapshot.clone(),
            composition: composition.cloned(),
        }
    }

    pub(crate) fn marked_ranges(&self) -> &[TextRange] {
        self.composition
            .as_ref()
            .map_or(&[], |composition| composition.ranges.as_ref())
    }

    pub(super) fn marked_utf16_range(&self) -> Option<Range<usize>> {
        let composition = self.composition.as_ref()?;
        let range = composition.ranges.get(composition.primary_index)?;
        Some(
            self.snapshot.byte_to_utf16_cu(range.start()).ok()?.get()
                ..self.snapshot.byte_to_utf16_cu(range.end()).ok()?.get(),
        )
    }

    fn text_for_utf16_range(&self, range: Range<usize>) -> Option<String> {
        let start = self
            .snapshot
            .utf16_cu_to_byte(Utf16Offset::new(range.start))
            .ok()?;
        let end = self
            .snapshot
            .utf16_cu_to_byte(Utf16Offset::new(range.end))
            .ok()?;
        self.snapshot
            .slice_byte_range(start, end)
            .ok()
            .map(|text| text.as_str().to_owned())
    }
}

impl Editor {
    fn selection_for_utf16_range(&self, range: Range<usize>, cx: &App) -> Option<SelectionSet> {
        let snapshot = self.buffer.read(cx).snapshot();
        let start = snapshot
            .utf16_cu_to_byte(Utf16Offset::new(range.start))
            .ok()?;
        let end = snapshot
            .utf16_cu_to_byte(Utf16Offset::new(range.end))
            .ok()?;
        Some(SelectionSet::new(vec![Selection::new(start, end)]))
    }

    pub(super) fn replace_text(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let before_selections = self.resolved_selections();
        let composition = self.composition.take();
        let Some((targets, text, metadata)) =
            self.commit_input_edit(composition.clone(), range_utf16, text, "输入文本", cx)
        else {
            return;
        };
        // 编辑统一入口负责提交与选区落位；失败时恢复组合会话（选区已由入口恢复）。
        if self
            .change_with_after(before_selections, cx, |buffer| {
                replace_selections(buffer, &targets, &text, metadata)
            })
            .is_err()
        {
            self.composition = composition;
        }
    }

    /// 输入编辑目标与元数据计算（普通输入与输入法组合共用）：替换目标、单行换行清洗与合并策略。
    ///
    /// `composition` 已在调用方 take；替换目标计算失败时原样恢复组合会话并返回 None。
    /// 文本编辑本身由调用方经 `Editor::change_with_after` 提交。
    fn commit_input_edit(
        &mut self,
        composition: Option<EditorComposition>,
        range_utf16: Option<Range<usize>>,
        text: &str,
        description: &'static str,
        cx: &mut Context<Self>,
    ) -> Option<(SelectionSet, String, TransactionMetadata)> {
        let targets = match self.replacement_targets(composition.as_ref(), range_utf16, cx) {
            Some(targets) => targets,
            None => {
                self.composition = composition;
                return None;
            }
        };
        let text = if self.mode == EditorMode::SingleLine {
            text.replace(['\r', '\n'], "")
        } else {
            text.to_owned()
        };
        let merge_with_composition = composition
            .as_ref()
            .and_then(|composition| composition.history_transaction_id)
            .is_some_and(|transaction_id| self.is_current_history_transaction(transaction_id, cx));
        Some((
            targets,
            text,
            input_metadata(description, merge_with_composition),
        ))
    }

    fn replacement_targets(
        &self,
        composition: Option<&EditorComposition>,
        range_utf16: Option<Range<usize>>,
        cx: &App,
    ) -> Option<SelectionSet> {
        if let Some(composition) = composition {
            let ranges = composition
                .ranges
                .iter()
                .copied()
                .map(|range| {
                    range_utf16.clone().map_or(Some(range), |relative_range| {
                        self.relative_utf16_range(range, relative_range, cx)
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            return Some(SelectionSet::new_with_primary(
                ranges
                    .into_iter()
                    .map(|range| Selection::new(range.start(), range.end()))
                    .collect(),
                composition.primary_index,
            ));
        }
        if let Some(range) = range_utf16 {
            return self.selection_for_utf16_range(range, cx);
        }
        Some(self.resolved_selections())
    }

    fn relative_utf16_range(
        &self,
        containing_range: TextRange,
        relative_range: Range<usize>,
        cx: &App,
    ) -> Option<TextRange> {
        let snapshot = self.buffer.read(cx).snapshot();
        let text = snapshot.slice_text(containing_range).ok()?;
        let text = text.as_str();
        let utf16_len = utf16_len(text);
        let start = byte_for_utf16_offset(text, relative_range.start.min(utf16_len))?;
        let end = byte_for_utf16_offset(text, relative_range.end.min(utf16_len))?;
        TextRange::new(
            ByteOffset::new(containing_range.start().get() + start),
            ByteOffset::new(containing_range.start().get() + end),
        )
        .ok()
    }

    fn is_current_history_transaction(&self, transaction_id: TransactionId, cx: &App) -> bool {
        let buffer = self.buffer.read(cx);
        buffer
            .current_history_node()
            .and_then(|node| buffer.history_node(node))
            .is_some_and(|node| node.transaction_id == transaction_id)
    }
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let presentation = self.presentation();
        actual_range.replace(range_utf16.clone());
        presentation.text_for_utf16_range(range_utf16)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let snapshot = self.display_map.buffer_snapshot();
        let selection = *self.resolved_selections().primary();
        Some(UTF16Selection {
            range: snapshot.byte_to_utf16_cu(selection.start()).ok()?.get()
                ..snapshot.byte_to_utf16_cu(selection.end()).ok()?.get(),
            reversed: selection.is_reversed(),
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.presentation().marked_utf16_range()
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.composition = None;
        self.input_layout = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text(range_utf16, text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous_composition = self.composition.take();
        let before_selections = self.resolved_selections();
        let Some((targets, text, metadata)) = self.commit_input_edit(
            previous_composition.clone(),
            range_utf16,
            new_text,
            "输入法组合",
            cx,
        ) else {
            return;
        };
        let previous_history_transaction = previous_composition
            .as_ref()
            .and_then(|composition| composition.history_transaction_id);
        let outcome = self.change_with_after(before_selections.clone(), cx, |buffer| {
            replace_selections(buffer, &targets, &text, metadata)
        });
        // 会话提交后当前历史节点即本次编辑的归属节点（MergeWithPrevious 时指向前节点），用它作为组合会话的事务身份：连续候选更新据此合并进同一撤销步。
        // 不能用编辑 outcome 的 history_transaction_id——会话 id 在合并进前节点后不指向任何历史节点，后续合并判断会失败。
        let buffer = self.buffer.read(cx);
        let history_transaction_id = buffer
            .current_history_node()
            .and_then(|node| buffer.history_node(node))
            .map(|node| node.transaction_id)
            .or(previous_history_transaction);
        if outcome.is_err() {
            self.composition = previous_composition;
            return;
        }
        if text.is_empty() {
            self.composition = None;
            return;
        }

        let inserted_selections = self.resolved_selections();
        let marked_ranges = inserted_selections
            .as_slice()
            .iter()
            .map(|selection| {
                let end = selection.head();
                let start = ByteOffset::new(end.get().saturating_sub(text.len()));
                TextRange::new(start, end).expect("替换后的选区必须能够还原出 marked text 范围")
            })
            .collect::<Vec<_>>();
        let text_utf16_len = utf16_len(&text);
        let selected_range_utf16 =
            new_selected_range_utf16.unwrap_or(text_utf16_len..text_utf16_len);
        let selected_start =
            byte_for_utf16_offset(&text, selected_range_utf16.start.min(text_utf16_len))
                .unwrap_or(text.len());
        let selected_end =
            byte_for_utf16_offset(&text, selected_range_utf16.end.min(text_utf16_len))
                .unwrap_or(text.len());
        let version = self.display_map.buffer_snapshot().version();
        self.selections = EditorSelections::from_selection_set(
            version,
            &SelectionSet::new_with_primary(
                marked_ranges
                    .iter()
                    .map(|marked_range| {
                        Selection::new(
                            ByteOffset::new(marked_range.start().get() + selected_start),
                            ByteOffset::new(marked_range.start().get() + selected_end),
                        )
                    })
                    .collect(),
                inserted_selections.primary_index(),
            ),
        );
        let selections = self.resolved_selections();
        if let Some(transaction_id) = history_transaction_id
            && let Some(transaction) = self.selection_history.transaction_mut(transaction_id)
        {
            // IME 组合期间同一事务的 redo 选区随候选更新推进。
            transaction.set_redo(selections);
        }
        self.composition = Some(EditorComposition {
            ranges: marked_ranges.into(),
            primary_index: inserted_selections.primary_index(),
            history_transaction_id,
        });
        self.request_autoscroll();
        self.input_layout = None;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let cursor = self.pixel_position_of_newest_cursor?;
        let bounds = self.last_bounds?;
        Some(Bounds::new(
            point(bounds.origin.x + cursor.x, bounds.origin.y + cursor.y),
            size(px(2.), self.last_line_height?),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.input_layout.as_ref()?.utf16_index_for_point(point)
    }
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

fn byte_for_utf16_offset(text: &str, target: usize) -> Option<usize> {
    let mut utf16_offset = 0;
    for (byte_offset, character) in text.char_indices() {
        if utf16_offset == target {
            return Some(byte_offset);
        }
        utf16_offset += character.len_utf16();
        if utf16_offset > target {
            return None;
        }
    }
    (utf16_offset == target).then_some(text.len())
}

/// 输入文本的合并元数据。
fn input_metadata(description: &'static str, merge_with_previous: bool) -> TransactionMetadata {
    let metadata =
        TransactionMetadata::new(TransactionSource::Programmatic).with_description(description);
    if merge_with_previous {
        metadata.with_merge_policy(TransactionMergePolicy::MergeWithPrevious)
    } else {
        metadata
    }
}
