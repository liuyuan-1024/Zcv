//! M7C / M7D：Command 执行器。
//!
//! 本模块是 Command -> Buffer 的适配层。它依赖 `command` 与 `buffer`，但 `buffer/`
//! 子模块不依赖 Command，从物理结构上保持“分层但不倒置”。

use crate::{
    CharOffset, Command, CommandContext, CommandOutcome, CommandSource, EngineResult, Line,
    LogicalColumn, MovementDirection, MovementUnit, Position, Selection, SelectionSet,
    TransactionMetadata, TransactionSource, buffer::Buffer,
};

impl Buffer {
    /// M7C：统一命令执行入口。
    ///
    /// 文本修改命令会走现有 Transaction 管线；纯移动命令只更新 `SelectionSet`，
    /// 不递增文本版本；Undo / Redo 复用历史系统；IME 命令复用 M6C composition 管线。
    pub fn execute_command(
        &mut self,
        command: Command,
        context: CommandContext,
    ) -> EngineResult<CommandOutcome> {
        let old_version = self.version();
        let old_selection = self.selection().clone();
        let old_composition = self.composition().cloned();
        let description = command.description();

        for _ in 0..repeat_count_for(&command, &context) {
            self.execute_command_once(&command, &context)?;
        }

        let new_version = self.version();
        let selection_changed = self.selection() != &old_selection;
        let composition_changed = self.composition().cloned() != old_composition;

        Ok(CommandOutcome::new(
            old_version,
            new_version,
            None,
            selection_changed,
            composition_changed,
            description,
        ))
    }

    fn execute_command_once(
        &mut self,
        command: &Command,
        context: &CommandContext,
    ) -> EngineResult<()> {
        if let Some((direction, unit, extend)) = command.as_unit_movement() {
            self.move_current_selection(direction, unit, extend)?;
            return Ok(());
        }

        if let Some((direction, extend)) = command.as_vertical_movement() {
            self.move_current_selection_vertically(direction, extend)?;
            return Ok(());
        }

        if let Some((is_start, extend)) = command.as_line_edge_movement() {
            let edge = if is_start {
                LineEdge::Start
            } else {
                LineEdge::End
            };
            self.move_current_selection_to_line_edge(edge, extend)?;
            return Ok(());
        }

        match command {
            Command::InsertText(text) | Command::ReplaceSelections(text) => {
                let selections = self.selection().clone();
                self.replace_selection_ranges_with_metadata(
                    selections,
                    text,
                    transaction_metadata_for(context, command),
                )?;
            }
            Command::DeleteSelection => {
                let selections = self.selection().clone();
                self.replace_selection_ranges_with_metadata(
                    selections,
                    "",
                    transaction_metadata_for(context, command),
                )?;
            }
            Command::DeleteBackward => {
                self.delete_by_movement(
                    MovementDirection::Previous,
                    MovementUnit::Grapheme,
                    transaction_metadata_for(context, command),
                )?;
            }
            Command::DeleteForward => {
                self.delete_by_movement(
                    MovementDirection::Next,
                    MovementUnit::Grapheme,
                    transaction_metadata_for(context, command),
                )?;
            }
            Command::DeleteWordBackward => {
                self.delete_by_movement(
                    MovementDirection::Previous,
                    MovementUnit::Word,
                    transaction_metadata_for(context, command),
                )?;
            }
            Command::DeleteWordForward => {
                self.delete_by_movement(
                    MovementDirection::Next,
                    MovementUnit::Word,
                    transaction_metadata_for(context, command),
                )?;
            }
            Command::DeleteSubwordBackward => {
                self.delete_by_movement(
                    MovementDirection::Previous,
                    MovementUnit::Subword,
                    transaction_metadata_for(context, command),
                )?;
            }
            Command::DeleteSubwordForward => {
                self.delete_by_movement(
                    MovementDirection::Next,
                    MovementUnit::Subword,
                    transaction_metadata_for(context, command),
                )?;
            }
            Command::SelectAll => {
                self.set_selection(SelectionSet::new(vec![Selection::new(
                    CharOffset::ZERO,
                    self.len_chars(),
                )]))?;
            }
            Command::ClearSelections => {
                let head = self.selection().primary().head();
                self.set_selection(SelectionSet::caret(head))?;
            }
            Command::SetSelections(selections) => {
                self.set_selection(selections.clone())?;
            }
            Command::AddSelection(selection) => {
                let mut selections = self.selection().as_slice().to_vec();
                let primary_index = selections.len();
                selections.push(*selection);
                self.set_selection(SelectionSet::new_with_primary(selections, primary_index))?;
            }
            Command::Undo => {
                self.undo()?;
            }
            Command::Redo => {
                self.redo()?;
            }
            Command::CompositionStart => {
                self.start_composition()?;
            }
            Command::CompositionUpdate(preedit_text) => {
                self.update_composition(preedit_text, None)?;
            }
            Command::CompositionCommit(commit_text) => {
                self.commit_composition(commit_text)?;
            }
            Command::CompositionCancel => {
                self.cancel_composition()?;
            }
            Command::MoveLeft { .. }
            | Command::MoveRight { .. }
            | Command::MoveUp { .. }
            | Command::MoveDown { .. }
            | Command::MoveLineStart { .. }
            | Command::MoveLineEnd { .. }
            | Command::MoveWordLeft { .. }
            | Command::MoveWordRight { .. }
            | Command::MoveIdentifierLeft { .. }
            | Command::MoveIdentifierRight { .. }
            | Command::MoveSubwordLeft { .. }
            | Command::MoveSubwordRight { .. }
            | Command::MoveSymbolLeft { .. }
            | Command::MoveSymbolRight { .. } => {
                unreachable!("movement commands are handled before match");
            }
        }

        Ok(())
    }

    fn delete_by_movement(
        &mut self,
        direction: MovementDirection,
        unit: MovementUnit,
        metadata: TransactionMetadata,
    ) -> EngineResult<()> {
        let selections = self.selection().clone();
        self.delete_by_movement_at_selections(selections, direction, unit, metadata)?;
        Ok(())
    }

    fn move_current_selection_to_line_edge(
        &mut self,
        edge: LineEdge,
        extend: bool,
    ) -> EngineResult<SelectionSet> {
        let selections = self.selection().clone();
        let primary_index = selections.primary_index();
        let moved = selections
            .as_slice()
            .iter()
            .copied()
            .map(|selection| {
                let position = self.char_to_position(selection.head())?;
                let new_head = match edge {
                    LineEdge::Start => self.line_start(position.line())?,
                    LineEdge::End => self.line_content_end(position.line())?,
                };

                Ok(if extend {
                    selection.with_head(new_head)
                } else {
                    Selection::caret(new_head)
                })
            })
            .collect::<EngineResult<Vec<_>>>()?;

        let moved = SelectionSet::new_with_primary(moved, primary_index);
        self.set_selection(moved.clone())?;
        Ok(moved)
    }

    fn move_current_selection_vertically(
        &mut self,
        direction: MovementDirection,
        extend: bool,
    ) -> EngineResult<SelectionSet> {
        let selections = self.selection().clone();
        let primary_index = selections.primary_index();
        let moved = selections
            .as_slice()
            .iter()
            .copied()
            .map(|selection| {
                let new_head = self.vertical_movement_target(selection.head(), direction)?;
                Ok(if extend {
                    selection.with_head(new_head)
                } else {
                    Selection::caret(new_head)
                })
            })
            .collect::<EngineResult<Vec<_>>>()?;

        let moved = SelectionSet::new_with_primary(moved, primary_index);
        self.set_selection(moved.clone())?;
        Ok(moved)
    }

    fn vertical_movement_target(
        &self,
        offset: CharOffset,
        direction: MovementDirection,
    ) -> EngineResult<CharOffset> {
        let position = self.char_to_position(offset)?;
        let current_line = position.line().get();
        let last_line = self.line_count().saturating_sub(1);
        let target_line = match direction {
            MovementDirection::Previous => current_line.saturating_sub(1),
            MovementDirection::Next => (current_line + 1).min(last_line),
        };

        if target_line == current_line {
            return Ok(offset);
        }

        let target_line = Line::new(target_line);
        let target_line_start = self.line_start(target_line)?;
        let target_line_end = self.line_content_end(target_line)?;
        let target_line_len = target_line_end
            .get()
            .saturating_sub(target_line_start.get());
        let target_column = position.column().get().min(target_line_len);

        self.position_to_char(Position::new(
            target_line,
            LogicalColumn::new(target_column),
        ))
    }

    fn line_content_end(&self, line: Line) -> EngineResult<CharOffset> {
        let next_line = line.get() + 1;
        if next_line >= self.line_count() {
            return Ok(self.len_chars());
        }

        let next_line_start = self.line_start(Line::new(next_line))?;
        self.previous_grapheme_boundary(next_line_start)
    }
}

fn repeat_count_for(command: &Command, context: &CommandContext) -> u32 {
    if command.is_repeatable() {
        context.repeat_count()
    } else {
        1
    }
}

fn transaction_metadata_for(context: &CommandContext, command: &Command) -> TransactionMetadata {
    TransactionMetadata::new(transaction_source_for(context.source(), command))
        .with_description(command.description())
}

fn transaction_source_for(source: CommandSource, command: &Command) -> TransactionSource {
    match command {
        Command::Undo => TransactionSource::Undo,
        Command::Redo => TransactionSource::Redo,
        _ if command.is_composition_command() => TransactionSource::Composition,
        _ if command.is_delete_command() => TransactionSource::Delete,
        _ => command_source_to_transaction_source(source),
    }
}

fn command_source_to_transaction_source(source: CommandSource) -> TransactionSource {
    match source {
        CommandSource::Keyboard => TransactionSource::Keyboard,
        CommandSource::Mouse => TransactionSource::Mouse,
        CommandSource::Paste => TransactionSource::Paste,
        CommandSource::CommandPalette | CommandSource::Menu => TransactionSource::Command,
        CommandSource::Ime => TransactionSource::Composition,
        CommandSource::Macro => TransactionSource::Macro,
        CommandSource::External => TransactionSource::External,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineEdge {
    Start,
    End,
}
