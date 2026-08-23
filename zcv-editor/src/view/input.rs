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
    ByteOffset, Selection, SelectionSet, Snapshot, Stickiness, TextRange, TrackedRange,
    TransactionId, Utf16Offset,
};

use super::*;
use crate::selection::{EditOutcome, EditorSelections, apply_edits, replace_selections};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorComposition {
    pub(super) ranges: Arc<[TextRange]>,
    pub(super) primary_index: usize,
    pub(super) history_transaction_id: Option<TransactionId>,
}

/// 自动补全的闭合符标记（对齐 Zed `AutocloseRegion`）。
///
/// 零宽 `TrackedRange` 锚在自动插入的闭合符起点（`Stickiness::Expand`）：
/// 向配对内输入文本时末端锚跟随闭合符右移，输入闭合符且光标紧贴末端时跳过，退格时光标贴着起点时删除整对。
#[derive(Debug, Clone, Copy)]
pub(crate) struct AutocloseRegion {
    pub(crate) range: TrackedRange,
    pub(crate) pair: AutoClosePair,
}

/// 输入行为编辑后落点的计算方式（以编辑前坐标为基准，编辑后在闭包内经 PositionMap 换算）。
enum AfterAction {
    /// 普通插入：光标落在插入文本末尾（替换非空选区时在替换文本末尾）。
    Insert { was_caret: bool },
    /// 自动补全：光标落在 open 之后、自动补全的 close 之前
    /// （map_old_position 在同点插入处返回插入文本之后，需回退 close 长度）。
    BetweenPair { close_len: usize },
    /// 跳过闭合符：不产生编辑，光标越过闭合符（落点 = 区域末端 + 闭合符长度）。
    SkipPast { end: ByteOffset, close_len: usize },
    /// 包裹选区：编辑后选区覆盖包裹后的文本（端点映射后回退 close 长度，不吸收闭合符）。
    Mapped { close_len: usize },
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
        // 自动闭合行为只作用于普通单字符输入（IME 组合会话与指定替换范围不进入）。
        if range_utf16.is_none()
            && composition.is_none()
            && self.try_auto_pair_input(text, &before_selections, cx)
        {
            return;
        }
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

    /// 自动闭合行为枢纽（对齐 Zed `handle_input` 的配对部分）。
    ///
    /// 逐选区决策：非空选区用配对包裹、光标贴着自动补全闭合符时跳过、键入 open 自动补 close；
    /// 任一选区命中行为时统一提交编辑并返回 true，否则返回 false 走普通插入路径。
    fn try_auto_pair_input(
        &mut self,
        text: &str,
        before: &SelectionSet,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(typed) = text.chars().next() else {
            return false;
        };
        // 自动闭合只处理单字符输入；多字符文本（粘贴等）不走此路径。
        if text.len() != typed.len_utf8() {
            return false;
        }
        let Some(pairs) = self.auto_close_pairs(cx) else {
            return false;
        };
        if pairs.is_empty() {
            return false;
        }
        let settings = SettingsStore::try_get(cx).unwrap_or_default();
        let auto_close_enabled = settings.use_autoclose;
        let auto_surround_enabled = settings.use_auto_surround;
        if !auto_close_enabled && !auto_surround_enabled {
            return false;
        }

        let before = before.normalized();
        let snapshot = self.buffer.read(cx).snapshot();

        // 逐选区决策，产出目标编辑、编辑后落点与新区域（以编辑前坐标为基准）。
        let mut targets: Vec<(Selection, Arc<str>)> = Vec::new();
        let mut after_actions: Vec<AfterAction> = Vec::new();
        let mut new_regions: Vec<TextRange> = Vec::new();
        let mut new_region_pairs: Vec<AutoClosePair> = Vec::new();
        let mut consumed = false;
        for selection in before.as_slice() {
            if !selection.is_caret() {
                if auto_surround_enabled
                    && let Some(pair) = pairs
                        .iter()
                        .find(|pair| pair.surround && pair.start == typed.to_string())
                {
                    targets.push((Selection::caret(selection.start()), Arc::from(pair.start)));
                    targets.push((Selection::caret(selection.end()), Arc::from(pair.end)));
                    after_actions.push(AfterAction::Mapped {
                        close_len: pair.end.len(),
                    });
                    consumed = true;
                } else {
                    targets.push((*selection, Arc::from(text)));
                    after_actions.push(AfterAction::Insert { was_caret: false });
                }
                continue;
            }
            if auto_close_enabled
                && let Some(region) = self.autoclose_region_at(selection.end(), typed, &snapshot)
            {
                after_actions.push(AfterAction::SkipPast {
                    end: region.range.range().end(),
                    close_len: region.pair.end.len(),
                });
                consumed = true;
                continue;
            }
            if auto_close_enabled
                && let Some(pair) = pairs
                    .iter()
                    .find(|pair| pair.close && pair.start == typed.to_string())
                && following_text_allows_autoclose(&snapshot, selection.end())
                && preceding_text_allows_autoclose(&snapshot, selection.end(), pair)
            {
                targets.push((*selection, Arc::from(format!("{text}{}", pair.end))));
                after_actions.push(AfterAction::BetweenPair {
                    close_len: pair.end.len(),
                });
                new_regions.push(
                    TextRange::new(selection.end(), selection.end()).expect("零宽区间必然合法"),
                );
                new_region_pairs.push(*pair);
                consumed = true;
                continue;
            }
            targets.push((*selection, Arc::from(text)));
            after_actions.push(AfterAction::Insert { was_caret: true });
        }
        if !consumed {
            return false;
        }

        // 统一提交：跳过场景不产生编辑，其余按目标文本插入；
        // 新区域在闭包内经 PositionMap 换算到编辑后坐标。
        let mut new_regions_after: Vec<(TextRange, AutoClosePair)> = Vec::new();
        let metadata = input_metadata("输入文本", false);
        let result = self.change_with_after(before.clone(), cx, |buffer| {
            let outcome = apply_edits(buffer, &targets, metadata.clone())?;
            let position_map = outcome
                .as_ref()
                .map(|transaction| transaction.event().position_map().clone())
                .unwrap_or_default();
            for (index, range) in new_regions.iter().enumerate() {
                // 区域锚在闭合符起点：光标经映射吸收到配对之后，回退 close 长度即闭合符起点。
                // （不能用零宽区间经 Expand 映射——同点插入会把整个配对吸进区间。）
                let mapped = position_map.map_old_position(range.start()).value();
                let close_start = ByteOffset::new(mapped.get() - new_region_pairs[index].end.len());
                new_regions_after.push((
                    TextRange::new(close_start, close_start).expect("零宽区间必然合法"),
                    new_region_pairs[index],
                ));
            }
            let after = SelectionSet::new_with_primary(
                before
                    .as_slice()
                    .iter()
                    .zip(after_actions.iter())
                    .map(|(selection, action)| match action {
                        AfterAction::Insert { was_caret } => {
                            let start = position_map.map_old_position(selection.start()).value();
                            Selection::caret(if *was_caret {
                                start
                            } else {
                                ByteOffset::new(start.get() + typed.len_utf8())
                            })
                        }
                        AfterAction::BetweenPair { close_len } => {
                            let start = position_map.map_old_position(selection.start()).value();
                            Selection::caret(ByteOffset::new(start.get() - close_len))
                        }
                        AfterAction::SkipPast { end, close_len } => {
                            let end = position_map.map_old_position(*end).value();
                            Selection::caret(ByteOffset::new(end.get() + close_len))
                        }
                        AfterAction::Mapped { close_len } => {
                            let start = position_map.map_old_position(selection.start()).value();
                            let end = position_map.map_old_position(selection.end()).value();
                            let end = ByteOffset::new(end.get() - close_len);
                            if selection.is_reversed() {
                                Selection::new(end, start)
                            } else {
                                Selection::new(start, end)
                            }
                        }
                    })
                    .collect(),
                before.primary_index(),
            );
            Ok((EditOutcome::from_transaction(outcome), after))
        });
        if result.is_ok() {
            let version = self.buffer.read(cx).snapshot().version();
            self.autoclose_regions
                .extend(
                    new_regions_after
                        .into_iter()
                        .map(|(range, pair)| AutocloseRegion {
                            range: TrackedRange::from_range(version, range, Stickiness::Expand),
                            pair,
                        }),
                );
        }
        true
    }

    /// 当前语言的自动闭合配对表。
    pub(super) fn auto_close_pairs(&self, cx: &App) -> Option<&'static [AutoClosePair]> {
        Some(self.language_buffer.read(cx).language()?.auto_close_pairs())
    }

    /// 光标处的待跳过自动闭合区域：区域末端锚与光标重合、该处文本确为配对闭合符。
    /// 嵌套配对取最内层（区域起点最大者）；区域版本滞后于当前快照（未跟踪的外部编辑）视为失效。
    fn autoclose_region_at(
        &self,
        end: ByteOffset,
        typed: char,
        snapshot: &Snapshot,
    ) -> Option<AutocloseRegion> {
        self.autoclose_regions
            .iter()
            .filter(|region| {
                region.range.version() == snapshot.version()
                    && region.range.range().end() == end
                    && region.pair.end == typed.to_string()
                    && text_at(snapshot, end, region.pair.end)
            })
            .max_by_key(|region| region.range.range().start())
            .copied()
    }

    /// 光标贴着自动补全闭合符起点时扩展选区覆盖整对（对齐 Zed `select_autoclose_pair`），使退格一次删除整对；非空选区或未命中区域时选区不变。
    pub(super) fn select_autoclose_pair(&mut self, cx: &App) {
        let snapshot = self.buffer.read(cx).snapshot();
        let before = self.resolved_selections();
        let mut changed = false;
        let selections: Vec<Selection> = before
            .as_slice()
            .iter()
            .map(|selection| {
                if !selection.is_caret() {
                    return *selection;
                }
                let Some(region) = self
                    .autoclose_regions
                    .iter()
                    .filter(|region| {
                        region.range.version() == snapshot.version()
                            && region.range.range().start() == selection.end()
                    })
                    .max_by_key(|region| region.range.range().start())
                    .copied()
                else {
                    return *selection;
                };
                let range = region.range.range();
                let Some(start) = range.start().get().checked_sub(region.pair.start.len()) else {
                    return *selection;
                };
                let start = ByteOffset::new(start);
                let Some(end) = range.end().checked_add(region.pair.end.len()) else {
                    return *selection;
                };
                // 校验开合文本确实位于区域两端，再扩展选区覆盖整对。
                if text_at(&snapshot, start, region.pair.start)
                    && text_at(&snapshot, range.end(), region.pair.end)
                {
                    changed = true;
                    Selection::new(start, end)
                } else {
                    *selection
                }
            })
            .collect();
        if changed {
            self.selections = EditorSelections::from_selection_set(
                snapshot.version(),
                &SelectionSet::new_with_primary(selections, before.primary_index()),
            );
        }
    }
}

/// 自动闭合的后续检查（对齐 Zed `autoclose_before`）：光标后是空白、行尾或常见语句分隔符时才自动闭合，避免在标识符前键入 open 时被自动补上 close。
const AUTOCLOSE_BEFORE: &str = ";:.,=}])>";

fn following_text_allows_autoclose(snapshot: &Snapshot, offset: ByteOffset) -> bool {
    let Ok((chunk, chunk_start)) = snapshot.chunk_at_byte(offset) else {
        return true;
    };
    let Some(next) = chunk[offset.get() - chunk_start.get()..].chars().next() else {
        return true;
    };
    next.is_whitespace() || AUTOCLOSE_BEFORE.contains(next)
}

/// 自动闭合的前置检查（对齐 Zed）：引号类配对（start == end）前是词字符时不自动闭合，避免在单词末尾输入引号时被当成新的开启引号。
fn preceding_text_allows_autoclose(
    snapshot: &Snapshot,
    offset: ByteOffset,
    pair: &AutoClosePair,
) -> bool {
    if pair.start != pair.end {
        return true;
    }
    let Ok((line, column)) = snapshot.byte_to_point(offset) else {
        return true;
    };
    if column == 0 {
        return true;
    }
    let Ok(line_slice) = snapshot.line_content(line, None) else {
        return true;
    };
    let prefix = &line_slice.as_str()[..column];
    !prefix
        .chars()
        .next_back()
        .is_some_and(|character| character.is_alphanumeric() || character == '_')
}

/// `offset` 处是否为指定文本（越界或文本不符返回 false）。
fn text_at(snapshot: &Snapshot, offset: ByteOffset, text: &str) -> bool {
    offset.checked_add(text.len()).is_some_and(|end| {
        snapshot
            .slice_byte_range(offset, end)
            .is_ok_and(|slice| slice.as_str() == text)
    })
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
