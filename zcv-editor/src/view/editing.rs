//! 编辑命令与选区历史：删除、缩进、换行、行移动、剪贴板与 Undo/Redo。
//!
//! 所有文本修改统一走编辑事务管线（`apply_edit_outcome` 族），handler 仅做参数翻译。

use std::collections::BTreeSet;
use std::sync::Arc;

use gpui::{App, ClipboardItem, Context, Window};
use zcv_engine::{
    ByteOffset, EngineResult, Line, MovementDirection, MovementUnit, Selection, SelectionSet,
    Snapshot,
};

use super::*;
use crate::selection::{
    EditorSelections, apply_edits_with_after_mapping, apply_targeted_edits, replace_selections,
};

impl Editor {
    fn delete(
        &mut self,
        direction: MovementDirection,
        unit: MovementUnit,
        description: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.composition = None;
        let before_selections = self.resolved_selections();
        let targets = self.delete_targets(&before_selections, Some((direction, unit)), cx);
        self.apply_deletion(before_selections, targets, description, cx);
    }

    fn delete_to_line_edge(
        &mut self,
        direction: MovementDirection,
        description: &'static str,
        cx: &mut Context<Self>,
    ) {
        self.composition = None;
        let before_selections = self.resolved_selections();
        let targets = {
            let buffer = self.buffer.read(cx);
            before_selections
                .as_slice()
                .iter()
                .map(|selection| {
                    let range = selection.range();
                    let pivot = match direction {
                        MovementDirection::Previous => range.start(),
                        MovementDirection::Next => range.end(),
                    };
                    let pivot_char = buffer.byte_to_char(pivot)?;
                    let mut boundary = buffer.char_to_byte(buffer.movement_boundary(
                        pivot_char,
                        direction,
                        MovementUnit::LineEdge,
                    )?)?;
                    if selection.is_caret() && boundary == pivot {
                        boundary = buffer.char_to_byte(buffer.movement_boundary(
                            pivot_char,
                            direction,
                            MovementUnit::Grapheme,
                        )?)?;
                    }
                    Ok(match direction {
                        MovementDirection::Previous => Selection::new(boundary, range.end()),
                        MovementDirection::Next => Selection::new(range.start(), boundary),
                    })
                })
                .collect::<EngineResult<Vec<_>>>()
                .map(SelectionSet::new)
        };
        self.apply_deletion(before_selections, targets, description, cx);
    }

    /// 删除命令尾部：删除目标即光标语义，编辑前重锚到 targets，编辑后端点塌缩到删除起点。
    fn apply_deletion(
        &mut self,
        before_selections: SelectionSet,
        targets: EngineResult<SelectionSet>,
        description: &'static str,
        cx: &mut Context<Self>,
    ) {
        let outcome = targets.and_then(|targets| {
            self.set_selections(targets.clone());
            self.buffer.update(cx, |buffer, cx| {
                let outcome = replace_selections(buffer, &targets, "", edit_metadata(description));
                cx.notify();
                outcome
            })
        });
        self.apply_edit_outcome(before_selections, outcome, cx);
    }

    /// 单行模式下的命令守卫：换行/缩进等编辑在单行输入框内由外层处理。
    pub(super) fn propagate_if_single_line(&self, cx: &mut Context<Self>) -> bool {
        if self.mode == EditorMode::SingleLine {
            cx.propagate();
            true
        } else {
            false
        }
    }

    fn delete_targets(
        &self,
        selections: &SelectionSet,
        caret_motion: Option<(MovementDirection, MovementUnit)>,
        cx: &App,
    ) -> EngineResult<SelectionSet> {
        let buffer = self.buffer.read(cx);
        let mut targets = Vec::new();
        for selection in selections.as_slice() {
            if !selection.is_caret() {
                targets.push(*selection);
                continue;
            }
            let Some((direction, unit)) = caret_motion else {
                continue;
            };
            let head_char = buffer.byte_to_char(selection.head())?;
            let boundary =
                buffer.char_to_byte(buffer.movement_boundary(head_char, direction, unit)?)?;
            if boundary != selection.head() {
                targets.push(match direction {
                    MovementDirection::Previous => Selection::new(boundary, selection.head()),
                    MovementDirection::Next => Selection::new(selection.head(), boundary),
                });
            }
        }
        if targets.is_empty() {
            Ok(selections.clone())
        } else {
            Ok(SelectionSet::new(targets))
        }
    }

    pub(super) fn indent(&mut self, cx: &mut Context<Self>) {
        if self.propagate_if_single_line(cx) {
            return;
        }
        let before = self.resolved_selections().normalized();
        let snapshot = self.buffer.read(cx).snapshot();
        let tab = snapshot.config().tab;
        let all_carets = before
            .as_slice()
            .iter()
            .all(|selection| selection.is_caret());
        let targets = if all_carets {
            before
                .as_slice()
                .iter()
                .map(|selection| {
                    let text: Arc<str> = if tab.insert_spaces {
                        let column = self
                            .display_map
                            .offset_to_display_point(selection.head())
                            .map_err(|error| zcv_engine::EngineError::EngineBug {
                                location: "Editor::indent",
                                detail: error.to_string(),
                            })?
                            .column()
                            .get();
                        let width = tab.indent_width();
                        Arc::from(" ".repeat(width - column % width))
                    } else {
                        Arc::from("\t")
                    };
                    Ok((*selection, text))
                })
                .collect::<EngineResult<Vec<_>>>()
        } else {
            touched_lines(&snapshot, &before).map(|lines| {
                let text: Arc<str> = if tab.insert_spaces {
                    Arc::from(" ".repeat(tab.indent_width()))
                } else {
                    Arc::from("\t")
                };
                lines
                    .into_iter()
                    .map(|line| {
                        (
                            Selection::caret(
                                snapshot
                                    .line_start_byte(line)
                                    .expect("已验证逻辑行必须有行首"),
                            ),
                            Arc::clone(&text),
                        )
                    })
                    .collect()
            })
        };
        let outcome = targets.and_then(|targets| {
            // 缩进目标即光标语义：编辑前把选区端点重锚到 targets。
            let target_selections =
                SelectionSet::new(targets.iter().map(|(selection, _)| *selection).collect());
            self.set_selections(target_selections);
            self.buffer.update(cx, |buffer, cx| {
                let outcome = apply_targeted_edits(buffer, targets, edit_metadata("增加缩进"));
                cx.notify();
                outcome
            })
        });
        self.apply_edit_outcome(before, outcome, cx);
    }

    pub(super) fn outdent(&mut self, cx: &mut Context<Self>) {
        if self.propagate_if_single_line(cx) {
            return;
        }
        let before = self.resolved_selections();
        let snapshot = self.buffer.read(cx).snapshot();
        let targets = touched_lines(&snapshot, &before).and_then(|lines| {
            lines
                .into_iter()
                .filter_map(|line| match leading_indent_range(&snapshot, line) {
                    Ok(Some(selection)) => Some(Ok((selection, Arc::from("")))),
                    Ok(None) => None,
                    Err(error) => Some(Err(error)),
                })
                .collect::<EngineResult<Vec<_>>>()
        });
        let outcome = targets.and_then(|targets| {
            let target_selections =
                SelectionSet::new(targets.iter().map(|(selection, _)| *selection).collect());
            self.set_selections(target_selections);
            self.buffer.update(cx, |buffer, cx| {
                let outcome = apply_targeted_edits(buffer, targets, edit_metadata("减少缩进"));
                cx.notify();
                outcome
            })
        });
        self.apply_edit_outcome(before, outcome, cx);
    }

    pub(super) fn insert_newline(&mut self, cx: &mut Context<Self>) {
        if self.propagate_if_single_line(cx) {
            return;
        }
        self.composition = None;
        let before = self.resolved_selections().normalized();
        let snapshot = self.buffer.read(cx).snapshot();
        let targets = before
            .as_slice()
            .iter()
            .map(|selection| {
                let offset = selection.start();
                let line = snapshot.byte_to_line(offset)?;
                let line_start = snapshot.line_start_byte(line)?;
                let prefix = snapshot.slice_byte_range(line_start, offset)?;
                let leading: String = prefix
                    .as_str()
                    .chars()
                    .take_while(|character| matches!(character, ' ' | '\t'))
                    .collect();
                let query_start = offset.get().saturating_sub(1);
                let query_end = offset
                    .get()
                    .saturating_add(1)
                    .min(snapshot.len_bytes().get());
                let should_indent = self
                    .syntax_snapshot
                    .indent_ranges(query_start..query_end, &snapshot)
                    .into_iter()
                    .any(|range| {
                        range.range.start < offset.get()
                            && offset.get() <= range.range.end
                            && range
                                .end
                                .as_ref()
                                .is_none_or(|end| offset.get() <= end.start)
                    });
                let indent = if should_indent {
                    if snapshot.config().tab.insert_spaces {
                        " ".repeat(snapshot.config().tab.indent_width())
                    } else {
                        "\t".to_owned()
                    }
                } else {
                    String::new()
                };
                Ok((*selection, Arc::from(format!("\n{leading}{indent}"))))
            })
            .collect::<EngineResult<Vec<_>>>();
        let outcome = targets.and_then(|targets| {
            let target_selections =
                SelectionSet::new(targets.iter().map(|(selection, _)| *selection).collect());
            self.set_selections(target_selections);
            self.buffer.update(cx, |buffer, cx| {
                let outcome = apply_targeted_edits(buffer, targets, edit_metadata("插入换行"));
                cx.notify();
                outcome
            })
        });
        self.apply_edit_outcome(before, outcome, cx);
    }

    fn selected_text(&self, cx: &App) -> Option<String> {
        let snapshot = self.buffer.read(cx).snapshot();
        let mut parts = Vec::new();
        for selection in self.resolved_selections().as_slice() {
            if selection.is_caret() {
                continue;
            }
            parts.push(
                snapshot
                    .slice_text(selection.range())
                    .ok()?
                    .as_str()
                    .to_owned(),
            );
        }
        (!parts.is_empty()).then(|| parts.join("\n"))
    }

    pub(super) fn undo(&mut self, cx: &mut Context<Self>) {
        self.replay_history(false, cx);
    }

    pub(super) fn redo(&mut self, cx: &mut Context<Self>) {
        self.replay_history(true, cx);
    }

    /// 回放文本历史（undo/redo）并恢复对应选区的共享实现。
    fn replay_history(&mut self, redo: bool, cx: &mut Context<Self>) {
        let action = if redo { "Redo" } else { "Undo" };
        let outcome = self.buffer.update(cx, |buffer, cx| {
            let outcome = if redo { buffer.redo() } else { buffer.undo() };
            cx.notify();
            outcome
        });
        match outcome {
            Ok(Some(outcome)) => {
                // 回放后文本与记录选区时相同，偏移快照可直接重锚定。
                if let Some(selections) = self
                    .selection_history
                    .transaction(outcome.transaction_id())
                    .map(|history| if redo { history.redo() } else { history.undo() }.clone())
                {
                    let version = self.buffer.read(cx).snapshot().version();
                    self.selections = EditorSelections::from_selection_set(version, &selections);
                }
                self.synchronize_after_history_edit(cx);
            }
            Ok(None) => {}
            Err(error) => eprintln!("Editor {action} 失败：{error}"),
        }
    }

    fn synchronize_after_history_edit(&mut self, cx: &mut Context<Self>) {
        self.composition = None;
        self.sync_display_map(cx);
        self.request_autoscroll();
        self.input_layout = None;
        cx.notify();
    }

    pub(crate) fn handle_backspace(
        &mut self,
        _: &Backspace,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete(
            MovementDirection::Previous,
            MovementUnit::Grapheme,
            "向后删除",
            cx,
        );
    }

    pub(crate) fn handle_delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.delete(
            MovementDirection::Next,
            MovementUnit::Grapheme,
            "向前删除",
            cx,
        );
    }

    pub(crate) fn handle_delete_to_previous_word_start(
        &mut self,
        _: &DeleteToPreviousWordStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete(
            MovementDirection::Previous,
            MovementUnit::Word,
            "删除到前一个词",
            cx,
        );
    }

    pub(crate) fn handle_delete_to_next_word_end(
        &mut self,
        _: &DeleteToNextWordEnd,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete(
            MovementDirection::Next,
            MovementUnit::Word,
            "删除到后一个词",
            cx,
        );
    }

    pub(crate) fn handle_delete_to_beginning_of_line(
        &mut self,
        _: &DeleteToBeginningOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_to_line_edge(MovementDirection::Previous, "删除到行首", cx);
    }

    pub(crate) fn handle_delete_to_end_of_line(
        &mut self,
        _: &DeleteToEndOfLine,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete_to_line_edge(MovementDirection::Next, "删除到行尾", cx);
    }

    pub(crate) fn handle_newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_newline(cx);
    }

    pub(crate) fn handle_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.undo(cx);
    }

    pub(crate) fn handle_redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.redo(cx);
    }

    pub(crate) fn handle_copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text(cx) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(crate) fn handle_cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.selected_text(cx) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.composition = None;
        let before_selections = self.resolved_selections();
        self.set_selections(before_selections.clone());
        let outcome = self.buffer.update(cx, |buffer, cx| {
            let outcome = replace_selections(buffer, &before_selections, "", edit_metadata("剪切"));
            cx.notify();
            outcome
        });
        self.apply_edit_outcome(before_selections, outcome, cx);
    }

    pub(crate) fn handle_paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let Some(text) = item.text() else {
            return;
        };
        if !text.is_empty() {
            self.replace_text(None, &text, cx);
        }
    }

    pub(crate) fn handle_indent(&mut self, _: &Indent, _: &mut Window, cx: &mut Context<Self>) {
        self.indent(cx);
    }

    pub(crate) fn handle_outdent(&mut self, _: &Outdent, _: &mut Window, cx: &mut Context<Self>) {
        self.outdent(cx);
    }

    pub(crate) fn handle_move_line_up(
        &mut self,
        _: &MoveLineUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_lines(MovementDirection::Previous, cx);
    }

    pub(crate) fn handle_move_line_down(
        &mut self,
        _: &MoveLineDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_lines(MovementDirection::Next, cx);
    }

    /// 把选区所在行块整体上移或下移一行。
    ///
    /// 上移：把行块前面一行的文本（含换行符）移到行块之后；
    /// 下移：把行块文本（含换行符）移到后面一行之后。
    /// 所有行块在同一个事务内完成，选区由 position_map 自动映射跟随行块。
    fn move_lines(&mut self, direction: MovementDirection, cx: &mut Context<Self>) {
        if self.propagate_if_single_line(cx) {
            return;
        }
        self.composition = None;
        let before = self.resolved_selections();
        let description = match direction {
            MovementDirection::Previous => "移动行到上方",
            MovementDirection::Next => "移动行到下方",
        };
        let snapshot = self.buffer.read(cx).snapshot();
        let outcome = line_blocks(&snapshot, &before)
            .and_then(|blocks| {
                let targets = move_line_targets(&snapshot, &blocks, direction)?;
                let plans = pending_selection_shift(&snapshot, &before, &blocks, direction)?;
                Ok((targets, plans))
            })
            .and_then(|(targets, plans)| {
                self.buffer.update(cx, |buffer, cx| {
                    let outcome = apply_edits_with_after_mapping(
                        buffer,
                        targets,
                        edit_metadata(description),
                        |snapshot| resolve_selection_shift(snapshot, &before, &plans),
                    );
                    cx.notify();
                    outcome
                })
            });
        self.apply_edit_outcome_with_after(before, outcome, cx);
    }
}

/// 选区涉及的行合并为不相邻的行块（相邻行并成一块），返回 (起始行, 末行)。
fn line_blocks(
    snapshot: &Snapshot,
    selections: &SelectionSet,
) -> EngineResult<Vec<(usize, usize)>> {
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    for line in touched_lines(snapshot, selections)? {
        let row = line.get();
        if let Some((_, end)) = blocks.last_mut()
            && row == *end + 1
        {
            *end = row;
        } else {
            blocks.push((row, row));
        }
    }
    Ok(blocks)
}

/// 行块末行行尾的字节偏移（含换行符；最后一行无换行则到文档末尾）。
fn line_block_end(snapshot: &Snapshot, end: usize) -> EngineResult<ByteOffset> {
    let line_count = snapshot.line_count();
    if end + 1 < line_count {
        snapshot.line_start_byte(Line::new(end + 1))
    } else {
        Ok(ByteOffset::new(snapshot.len_bytes().get()))
    }
}

/// 行内容末尾的字节偏移（不含换行符）。
fn line_content_end(snapshot: &Snapshot, line: usize) -> EngineResult<ByteOffset> {
    let end = line_block_end(snapshot, line)?;
    if line + 1 < snapshot.line_count() {
        Ok(ByteOffset::new(end.get().saturating_sub(1)))
    } else {
        Ok(end)
    }
}

/// 生成行移动的编辑目标。
///
/// 上移把前面一行移到行块后，下移把行块移到后面一行后。
fn move_line_targets(
    snapshot: &Snapshot,
    blocks: &[(usize, usize)],
    direction: MovementDirection,
) -> EngineResult<Vec<(Selection, Arc<str>)>> {
    let line_count = snapshot.line_count();
    // 只处理实际会移动的行块：首行不能上移、末行不能下移。
    // 选区平移也必须基于这份子集，否则 no-op 行块的端点会越界。
    let movable = blocks
        .iter()
        .copied()
        .filter(|&(start, end)| match direction {
            MovementDirection::Previous => start > 0,
            MovementDirection::Next => end + 1 < line_count,
        })
        .collect::<Vec<_>>();
    let mut targets = Vec::new();
    for (start, end) in &movable {
        let (start, end) = (*start, *end);
        // 删除交换方整行（含换行符），在对方行尾（不含换行）插入前导换行 + 交换方内容：末行没有换行符，只能靠前导换行分隔。
        match direction {
            MovementDirection::Previous => {
                let previous_start = snapshot.line_start_byte(Line::new(start - 1))?;
                let previous_end = snapshot.line_start_byte(Line::new(start))?;
                let content = snapshot
                    .slice_byte_range(previous_start, line_content_end(snapshot, start - 1)?)?;
                let insertion = line_content_end(snapshot, end)?;
                targets.push((Selection::new(previous_start, previous_end), Arc::from("")));
                targets.push((
                    Selection::caret(insertion),
                    Arc::from(format!("\n{}", content.as_str())),
                ));
            }
            MovementDirection::Next => {
                let block_start = snapshot.line_start_byte(Line::new(start))?;
                let block_end = line_block_end(snapshot, end)?;
                let content =
                    snapshot.slice_byte_range(block_start, line_content_end(snapshot, end)?)?;
                let insertion = line_content_end(snapshot, end + 1)?;
                targets.push((Selection::new(block_start, block_end), Arc::from("")));
                targets.push((
                    Selection::caret(insertion),
                    Arc::from(format!("\n{}", content.as_str())),
                ));
            }
        }
    }
    Ok(targets)
}

/// 编辑前记录每个选区端点的 (行内字节偏移, 目标行号)，供编辑后定位。
///
/// 行内容整体移动，行内字节偏移编辑前后一致；
/// 行号平移只在端点行属于实际移动的行块时发生（选区端点所在行必有选区，理论上一概在行块内）。
fn pending_selection_shift(
    snapshot: &Snapshot,
    selections: &SelectionSet,
    blocks: &[(usize, usize)],
    direction: MovementDirection,
) -> EngineResult<Vec<(usize, usize)>> {
    let delta = match direction {
        MovementDirection::Previous => -1i64,
        MovementDirection::Next => 1i64,
    };
    selections
        .as_slice()
        .iter()
        .flat_map(|selection| [selection.anchor(), selection.head()])
        .map(|offset| {
            let line = snapshot.byte_to_line(offset)?.get();
            let line_start = snapshot.line_start_byte(Line::new(line))?.get();
            let target_line = if blocks
                .iter()
                .any(|(start, end)| line >= *start && line <= *end)
            {
                (line as i64 + delta) as usize
            } else {
                line
            };
            Ok((offset.get() - line_start, target_line))
        })
        .collect()
}

/// 按编辑后的快照把 (行内偏移, 目标行) 还原为字节偏移；新行较短时钳制到行尾。
fn resolve_selection_shift(
    snapshot: &Snapshot,
    selections: &SelectionSet,
    plans: &[(usize, usize)],
) -> EngineResult<SelectionSet> {
    let shifted = selections
        .as_slice()
        .iter()
        .zip(plans.chunks(2))
        .map(|(selection, plan)| {
            let anchor = resolve_point(snapshot, plan[0])?;
            let head = resolve_point(snapshot, plan[1])?;
            Ok(Selection::new(anchor, head).with_goal(selection.goal()))
        })
        .collect::<EngineResult<Vec<_>>>()?;
    Ok(SelectionSet::new_with_primary(
        shifted,
        selections.primary_index(),
    ))
}

fn resolve_point(
    snapshot: &Snapshot,
    (offset_in_line, target_line): (usize, usize),
) -> EngineResult<ByteOffset> {
    let line_start = snapshot.line_start_byte(Line::new(target_line))?.get();
    let content_len = line_content_end(snapshot, target_line)?.get() - line_start;
    Ok(ByteOffset::new(
        line_start + offset_in_line.min(content_len),
    ))
}

pub(super) fn touched_lines(
    snapshot: &Snapshot,
    selections: &SelectionSet,
) -> EngineResult<Vec<Line>> {
    let mut lines = BTreeSet::new();
    for selection in selections.as_slice() {
        let range = selection.range();
        let start = snapshot.byte_to_line(range.start())?;
        let mut end = snapshot.byte_to_line(range.end())?;
        if !range.is_empty() && end > start && snapshot.line_start_byte(end)? == range.end() {
            end = Line::new(end.get() - 1);
        }
        lines.extend((start.get()..=end.get()).map(Line::new));
    }
    Ok(lines.into_iter().collect())
}

fn leading_indent_range(snapshot: &Snapshot, line: Line) -> EngineResult<Option<Selection>> {
    let start = snapshot.line_start_byte(line)?;
    let text = snapshot.slice_line(line)?;
    let content = text.as_str();
    let end = if content.starts_with('\t') {
        start.checked_add(1)
    } else {
        let spaces = content
            .bytes()
            .take(snapshot.config().tab.indent_width())
            .take_while(|byte| *byte == b' ')
            .count();
        start.checked_add(spaces)
    };
    Ok(end
        .filter(|end| *end > start)
        .map(|end| Selection::new(start, end)))
}
