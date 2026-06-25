//! 视觉行移动。
//!
//! `editor.move_selection` 的命令语义是「按用户当前看到的行移动」。
//! 视觉模型由 zom-engine 的 [`WrapMap`] 提供：渲染端按字体度量算好行内断点，整篇同步落到 view；
//! 本模块只在文本域查询。
//!
//! sticky 用「视觉列」（grapheme 数），不依赖像素 x，垂直移动模型与帧渲染节奏完全解耦。
//! 当 `wrap_map` 为空（首帧未送达，或对该 target 不适用）时回退到 engine 的逻辑行移动；
//! WrapMap 在不软换行时 breaks 列表为空，行为天然退化为逻辑行；
//! 本模块仅保留处理 wrap_map 未就绪的极端情况。

use zom_engine::{Motion, MovementDirection, MovementUnit, Selection, SelectionSet};
use zom_workspace::view::{VisualAffinity, VisualPosition, WrapMap};

use crate::{CommandError, command_execution_failed};

/// 统一的 selection 移动入口。
pub(super) fn move_target_selection(
    mut target: crate::EditTarget<'_>,
    selections: SelectionSet,
    direction: MovementDirection,
    motion: Motion,
    extend: bool,
) -> Result<(), CommandError> {
    if let Some(wrap_map) = target.wrap_map {
        match motion {
            Motion::LineStep | Motion::PageStep { .. } => {
                let n = match motion {
                    Motion::LineStep => 1,
                    Motion::PageStep { lines } => lines.max(1),
                    Motion::ByUnit(_) => unreachable!(),
                };
                return apply_visual_row_step(
                    &mut target,
                    wrap_map,
                    &selections,
                    direction,
                    n,
                    extend,
                );
            }
            Motion::ByUnit(MovementUnit::LineEdge) => {
                return apply_visual_line_edge(
                    &mut target,
                    wrap_map,
                    &selections,
                    direction,
                    extend,
                );
            }
            Motion::ByUnit(MovementUnit::Grapheme) => {
                // 仅当 primary caret 处于软换行边界（同一 byte 跨段）时，wrap-aware 才有意义；
                // 否则 grapheme 步进的字节量与逻辑路径完全一致，留给 engine 走通用路径。
                if try_grapheme_wrap_boundary(
                    &mut target,
                    wrap_map,
                    &selections,
                    direction,
                    extend,
                )? {
                    return Ok(());
                }
            }
            Motion::ByUnit(_) => {}
        }
    }

    if !matches!(motion, Motion::LineStep | Motion::PageStep { .. }) {
        target.clear_visual_caret();
    }
    let moved = target
        .buffer
        .move_selections(selections, direction, motion, extend)
        .map_err(command_execution_failed)?;
    target.set_selection_preserving_visual_state(moved)?;
    Ok(())
}

/// 取 primary caret 当前的 VisualPosition。
///
/// 若 view 缓存了 visual_caret 且 byte 与 selection.head 一致，直接复用——
/// 这是保留 affinity 与 sticky 列的关键路径，连续上下移动不会被中途的解析歧义打断。
/// 缓存对不上时按 WrapMap 默认规则（边界归属下一段行首）重新解析。
fn primary_visual_position(
    wrap_map: &WrapMap,
    buffer: &zom_engine::Buffer,
    head: zom_engine::ByteOffset,
    cached: Option<&VisualPosition>,
) -> Result<VisualPosition, CommandError> {
    if let Some(pos) = cached
        && pos.byte == head
    {
        return Ok(*pos);
    }
    wrap_map
        .resolve(buffer, head, None)
        .map_err(command_execution_failed)
}

fn apply_visual_row_step(
    target: &mut crate::EditTarget<'_>,
    wrap_map: &WrapMap,
    selections: &SelectionSet,
    direction: MovementDirection,
    n: u32,
    extend: bool,
) -> Result<(), CommandError> {
    let primary_index = selections.primary_index();
    let primary = selections.primary();

    let cached_caret = target
        .visual_caret
        .as_deref()
        .and_then(|c| c.as_ref())
        .copied();
    let primary_start = primary_visual_position(
        wrap_map,
        target.buffer,
        primary.head(),
        cached_caret.as_ref(),
    )?;

    // sticky 列：连续垂直移动时使用第一次移动落定的列。
    let goal_column = target
        .goal_column
        .as_deref()
        .and_then(|g| *g)
        .unwrap_or(primary_start.column);

    let mut next_primary_caret = None;

    let moved = selections
        .as_slice()
        .iter()
        .enumerate()
        .map(|(index, selection)| {
            let selection = *selection;
            let start = if index == primary_index {
                primary_start
            } else {
                match wrap_map.resolve(target.buffer, selection.head(), None) {
                    Ok(p) => p,
                    Err(_) => return selection,
                }
            };
            let column = if index == primary_index {
                goal_column
            } else {
                start.column
            };
            let next = match wrap_map.step_visual_row(target.buffer, start, direction, n, column) {
                Ok(p) => p,
                Err(_) => return selection,
            };
            if index == primary_index {
                next_primary_caret = Some(next);
            }
            if extend {
                selection.with_head(next.byte)
            } else {
                Selection::caret(next.byte)
            }
        })
        .collect::<Vec<_>>();

    let moved = SelectionSet::new_with_primary(moved, primary_index);
    target.set_selection_preserving_visual_state(moved)?;

    if let Some(caret) = target.visual_caret.as_deref_mut() {
        *caret = next_primary_caret;
    }
    if let Some(goal) = target.goal_column.as_deref_mut() {
        *goal = Some(goal_column);
    }
    Ok(())
}

fn apply_visual_line_edge(
    target: &mut crate::EditTarget<'_>,
    wrap_map: &WrapMap,
    selections: &SelectionSet,
    direction: MovementDirection,
    extend: bool,
) -> Result<(), CommandError> {
    let primary_index = selections.primary_index();
    let cached_caret = target
        .visual_caret
        .as_deref()
        .and_then(|c| c.as_ref())
        .copied();

    let mut next_primary_caret = None;
    let moved = selections
        .as_slice()
        .iter()
        .enumerate()
        .map(|(index, selection)| {
            let selection = *selection;
            let cache = if index == primary_index {
                cached_caret.as_ref()
            } else {
                None
            };
            let start =
                match primary_visual_position(wrap_map, target.buffer, selection.head(), cache) {
                    Ok(p) => p,
                    Err(_) => return selection,
                };
            let end = match wrap_map.visual_line_edge(target.buffer, start, direction) {
                Ok(p) => p,
                Err(_) => return selection,
            };
            if index == primary_index {
                next_primary_caret = Some(end);
            }
            if extend {
                selection.with_head(end.byte)
            } else {
                Selection::caret(end.byte)
            }
        })
        .collect::<Vec<_>>();

    let moved = SelectionSet::new_with_primary(moved, primary_index);
    target.set_selection_preserving_visual_state(moved)?;
    if let Some(caret) = target.visual_caret.as_deref_mut() {
        *caret = next_primary_caret;
    }
    // LineEdge 不参与垂直 sticky，清掉。
    if let Some(goal) = target.goal_column.as_deref_mut() {
        *goal = None;
    }
    Ok(())
}

/// 处理 primary caret 正好在软换行边界、单 grapheme 步进需要原地翻转 affinity 的情形。
///
/// 返回 `Ok(true)` 表示已在边界翻转完成；
/// 返回 `Ok(false)` 表示 caret 不在边界，调用方按 engine 的常规 grapheme 步进路径处理。
fn try_grapheme_wrap_boundary(
    target: &mut crate::EditTarget<'_>,
    wrap_map: &WrapMap,
    selections: &SelectionSet,
    direction: MovementDirection,
    extend: bool,
) -> Result<bool, CommandError> {
    let cached = target
        .visual_caret
        .as_deref()
        .and_then(|c| c.as_ref())
        .copied();
    let primary = selections.primary();
    let primary_pos =
        primary_visual_position(wrap_map, target.buffer, primary.head(), cached.as_ref())?;

    let need_boundary = match direction {
        MovementDirection::Previous => {
            primary_pos.affinity == VisualAffinity::LineStart && primary_pos.subrow > 0
        }
        MovementDirection::Next => {
            primary_pos.affinity == VisualAffinity::LineEnd
                && primary_pos.subrow + 1 < wrap_map.subrow_count(primary_pos.logical_line)
        }
    };
    if !need_boundary {
        return Ok(false);
    }

    let next = wrap_map
        .grapheme(target.buffer, primary_pos, direction)
        .map_err(command_execution_failed)?;
    let Some(next) = next else {
        return Ok(false);
    };
    // 边界跨段时 byte 不变；只更新 visual_caret，selection 不动。
    if extend {
        // extend：保持现状（byte 没变）即可。
    } else if next.byte != primary.head() {
        // 兜底：理论上 byte 应保持不变，若不一致也尊重 wrap_map 的结果。
        let new_primary = Selection::caret(next.byte);
        let primary_index = selections.primary_index();
        let mut updated = selections.as_slice().to_vec();
        updated[primary_index] = new_primary;
        let moved = SelectionSet::new_with_primary(updated, primary_index);
        target.set_selection_preserving_visual_state(moved)?;
    }
    if let Some(caret) = target.visual_caret.as_deref_mut() {
        *caret = Some(next);
    }
    if let Some(goal) = target.goal_column.as_deref_mut() {
        *goal = None;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zom_engine::{Buffer, BufferConfig, ByteOffset, SelectionSet};

    fn b(value: usize) -> ByteOffset {
        ByteOffset::new(value)
    }

    fn make_target<'a>(
        buffer: &'a mut Buffer,
        selection: &'a mut SelectionSet,
        wrap_map: Option<&'a WrapMap>,
        visual_caret: &'a mut Option<VisualPosition>,
        goal_column: &'a mut Option<u32>,
    ) -> crate::EditTarget<'a> {
        crate::EditTarget {
            buffer,
            selection,
            wrap_map,
            visual_caret: Some(visual_caret),
            goal_column: Some(goal_column),
        }
    }

    fn map_3_3_3() -> WrapMap {
        WrapMap::new(true, vec![vec![3, 6]])
    }

    #[test]
    fn line_step_down_moves_to_next_visual_row_same_column() {
        let mut buffer = Buffer::from_text("abcdefghi".into(), BufferConfig::default()).unwrap();
        let mut selection = SelectionSet::caret(b(1));
        buffer.set_selection(selection.clone()).unwrap();
        let map = map_3_3_3();
        let mut caret = None;
        let mut goal = None;
        let target = make_target(
            &mut buffer,
            &mut selection,
            Some(&map),
            &mut caret,
            &mut goal,
        );
        move_target_selection(
            target,
            SelectionSet::caret(b(1)),
            MovementDirection::Next,
            Motion::LineStep,
            false,
        )
        .unwrap();
        assert_eq!(selection.primary().head(), b(4));
        assert_eq!(goal, Some(1));
        assert_eq!(caret.unwrap().subrow, 1);
    }

    #[test]
    fn line_step_down_sticky_column_preserved_across_multiple_steps() {
        let mut buffer = Buffer::from_text("abcdefghi".into(), BufferConfig::default()).unwrap();
        let mut selection = SelectionSet::caret(b(2));
        buffer.set_selection(selection.clone()).unwrap();
        let map = map_3_3_3();
        let mut caret = None;
        let mut goal = None;
        // 第一步：先建立 sticky = 2
        move_target_selection(
            make_target(
                &mut buffer,
                &mut selection,
                Some(&map),
                &mut caret,
                &mut goal,
            ),
            SelectionSet::caret(b(2)),
            MovementDirection::Next,
            Motion::LineStep,
            false,
        )
        .unwrap();
        assert_eq!(selection.primary().head(), b(5));
        // 第二步：sticky 仍是 2
        let snapshot = selection.clone();
        move_target_selection(
            make_target(
                &mut buffer,
                &mut selection,
                Some(&map),
                &mut caret,
                &mut goal,
            ),
            snapshot,
            MovementDirection::Next,
            Motion::LineStep,
            false,
        )
        .unwrap();
        assert_eq!(selection.primary().head(), b(8));
        assert_eq!(goal, Some(2));
    }

    #[test]
    fn line_edge_end_lands_on_subrow_end() {
        let mut buffer = Buffer::from_text("abcdefghi".into(), BufferConfig::default()).unwrap();
        let mut selection = SelectionSet::caret(b(4));
        buffer.set_selection(selection.clone()).unwrap();
        let map = map_3_3_3();
        let mut caret = None;
        let mut goal = None;
        move_target_selection(
            make_target(
                &mut buffer,
                &mut selection,
                Some(&map),
                &mut caret,
                &mut goal,
            ),
            SelectionSet::caret(b(4)),
            MovementDirection::Next,
            Motion::ByUnit(MovementUnit::LineEdge),
            false,
        )
        .unwrap();
        assert_eq!(selection.primary().head(), b(6));
        assert_eq!(caret.unwrap().affinity, VisualAffinity::LineEnd);
    }

    #[test]
    fn grapheme_next_at_wrap_boundary_crosses_without_moving_byte() {
        let mut buffer = Buffer::from_text("abcdefghi".into(), BufferConfig::default()).unwrap();
        let mut selection = SelectionSet::caret(b(3));
        buffer.set_selection(selection.clone()).unwrap();
        let map = map_3_3_3();
        let mut caret = Some(
            map.resolve(&buffer, b(3), Some(VisualAffinity::LineEnd))
                .unwrap(),
        );
        let mut goal = None;
        move_target_selection(
            make_target(
                &mut buffer,
                &mut selection,
                Some(&map),
                &mut caret,
                &mut goal,
            ),
            SelectionSet::caret(b(3)),
            MovementDirection::Next,
            Motion::ByUnit(MovementUnit::Grapheme),
            false,
        )
        .unwrap();
        // byte 不动，affinity 翻转到 LineStart。
        assert_eq!(selection.primary().head(), b(3));
        let after = caret.unwrap();
        assert_eq!(after.subrow, 1);
        assert_eq!(after.affinity, VisualAffinity::LineStart);
    }

    #[test]
    fn grapheme_next_off_boundary_falls_through_to_engine() {
        let mut buffer = Buffer::from_text("abcdefghi".into(), BufferConfig::default()).unwrap();
        let mut selection = SelectionSet::caret(b(1));
        buffer.set_selection(selection.clone()).unwrap();
        let map = map_3_3_3();
        let mut caret = None;
        let mut goal = None;
        move_target_selection(
            make_target(
                &mut buffer,
                &mut selection,
                Some(&map),
                &mut caret,
                &mut goal,
            ),
            SelectionSet::caret(b(1)),
            MovementDirection::Next,
            Motion::ByUnit(MovementUnit::Grapheme),
            false,
        )
        .unwrap();
        assert_eq!(selection.primary().head(), b(2));
    }

    #[test]
    fn fallback_to_engine_when_wrap_map_absent() {
        let mut buffer = Buffer::from_text("abc\nxyz".into(), BufferConfig::default()).unwrap();
        let mut selection = SelectionSet::caret(b(1));
        buffer.set_selection(selection.clone()).unwrap();
        let mut caret = None;
        let mut goal = None;
        let target = make_target(&mut buffer, &mut selection, None, &mut caret, &mut goal);
        move_target_selection(
            target,
            SelectionSet::caret(b(1)),
            MovementDirection::Next,
            Motion::LineStep,
            false,
        )
        .unwrap();
        // engine 逻辑行移动：col=1 → 下一行 col=1。
        assert_eq!(selection.primary().head(), b(5));
    }
}
