//! 跳转到行命令。

use crate::commands::cid;
use crate::commands::system::dismiss as dismiss_top;
use crate::{
    CommandArgs, CommandContext, CommandError, CommandOutcome, CommandRegistry, DismissScope,
    GoToLineEffect, HostEffect, Invocation, KeyBindingContext, Keymap, NoArgs,
    command_execution_failed,
};
use zom_engine::{ByteOffset, Line, TextRange};

pub const ACTIVATE: &str = "editor.go_to_line";
pub const DISMISS: &str = "editor.go_to_line_dismiss";
pub const CONFIRM: &str = "editor.go_to_line_confirm";

pub fn activate() -> Invocation {
    (cid(ACTIVATE), CommandArgs::new())
}

fn dismiss() -> Invocation {
    (cid(DISMISS), CommandArgs::new())
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    let text_edit = KeyBindingContext::text_edit();
    let go_to_line_input = KeyBindingContext::go_to_line_input();

    registry
        .install(keymap, ACTIVATE, "跳转到行", Box::new(run_activate))
        .description("跳转到指定行。")
        .key_in("mod g", text_edit);

    registry
        .install(keymap, DISMISS, "退出跳转到行", Box::new(run_dismiss))
        .hide_from_shortcuts();

    registry
        .install(keymap, CONFIRM, "确认跳转到行", Box::new(run_confirm))
        .hide_from_shortcuts()
        .key_in("enter", go_to_line_input)
        .key_in("return", go_to_line_input);

    dismiss_top::bind_esc(
        keymap,
        DismissScope::GoToLineInput,
        KeyBindingContext::go_to_line_input(),
    );
}

fn run_activate(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.dismiss.clear(DismissScope::GoToLineInput);
    context
        .dismiss
        .push(DismissScope::GoToLineInput, "退出跳转到行", dismiss());
    context
        .effects
        .push(HostEffect::GoToLine(GoToLineEffect::Activate));
    Ok(CommandOutcome::default())
}

fn run_dismiss(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.dismiss.clear(DismissScope::GoToLineInput);
    context
        .effects
        .push(HostEffect::GoToLine(GoToLineEffect::Dismiss));
    Ok(CommandOutcome::default())
}

fn run_confirm(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;

    // 从 go_to_line 输入框读行号文本，解析 "行:列" / "行,列" / "行 列" 等格式
    let line_text = {
        let target = context.edit_target()?;
        let range = TextRange::new(ByteOffset::ZERO, target.buffer.len_bytes())
            .map_err(|e| command_execution_failed(e.into()))?;
        target
            .buffer
            .slice_text(range)
            .map_err(command_execution_failed)?
            .to_string()
    };

    let (line_number, column_number) = parse_line_column(line_text.trim())?;

    let view_id = context.active_view_id.ok_or(CommandError::NoActiveView)?;
    let buffer_id = context
        .views
        .edit_view(view_id)
        .map(|v| v.buffer())
        .ok_or(CommandError::NoActiveView)?;
    let buffer = context
        .workspace
        .buffer(buffer_id)
        .ok_or(CommandError::BufferNotFound(buffer_id))?;
    let buf = buffer.buffer();

    let line = if line_number > 0 {
        Line::new(line_number - 1)
    } else {
        Line::ZERO
    };
    let total_lines = buf.line_count();
    let line = Line::new(line.get().min(total_lines.saturating_sub(1)));

    // 跳到目标列（0-based），超出行长则落到行尾
    let target_byte = {
        let line_start = buf
            .line_start_byte(line)
            .map_err(command_execution_failed)?;
        let line_end = buf
            .line_start_byte(Line::new(line.get() + 1))
            .unwrap_or(buf.len_bytes());
        let mut byte = line_start;
        for _ in 0..column_number.saturating_sub(1) {
            let Ok(next) = buf.next_grapheme_boundary_byte(byte) else {
                break;
            };
            if next >= line_end {
                break;
            }
            byte = next;
        }
        byte
    }
    .get();

    context.dismiss.clear(DismissScope::GoToLineInput);
    context
        .effects
        .push(HostEffect::GoToLine(GoToLineEffect::Jump(target_byte)));

    Ok(CommandOutcome::default())
}

/// 从 "行:列" / "行,列" / "行 列" 等格式中解析 1-based 行号和列号。
/// 无列时列号默认为 1。
fn parse_line_column(input: &str) -> Result<(usize, usize), CommandError> {
    // 找第一个数字序列
    let first_start = input
        .find(|c: char| c.is_ascii_digit())
        .ok_or_else(|| CommandError::InvalidArgs(format!("无效行号：{input}")))?;
    let first_end = input[first_start..]
        .find(|c: char| !c.is_ascii_digit())
        .map(|p| first_start + p)
        .unwrap_or(input.len());
    let line: usize = input[first_start..first_end]
        .parse()
        .map_err(|_| CommandError::InvalidArgs(format!("无效行号：{input}")))?;

    // 跳过非数字分隔符，找第二个数字序列
    let after_first = &input[first_end..];
    let second_start = after_first.find(|c: char| c.is_ascii_digit());
    let column = match second_start {
        None => 1,
        Some(pos) => {
            let start = first_end + pos;
            let end = input[start..]
                .find(|c: char| !c.is_ascii_digit())
                .map(|p| start + p)
                .unwrap_or(input.len());
            input[start..end]
                .parse()
                .map_err(|_| CommandError::InvalidArgs(format!("无效行号：{input}")))?
        }
    };

    Ok((line, column))
}
