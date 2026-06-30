//! 命令 catalog 共享的参数解析辅助函数。

use zom_engine::EngineError;

use crate::{CommandArgs, CommandError};

/// 通用移动步长参数，被 file_tree、version_control、project_picker 共享。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MoveDeltaArgs {
    pub delta: isize,
}

impl From<MoveDeltaArgs> for CommandArgs {
    fn from(args: MoveDeltaArgs) -> Self {
        CommandArgs::new().with("delta", args.delta.to_string())
    }
}

impl TryFrom<CommandArgs> for MoveDeltaArgs {
    type Error = CommandError;

    fn try_from(args: CommandArgs) -> Result<Self, Self::Error> {
        reject_unknown_args(&args, &["delta"])?;
        let raw = required_arg(&args, "delta")?;
        let delta = raw
            .parse()
            .map_err(|_| CommandError::InvalidArgs(format!("无效移动步长：{raw}")))?;
        Ok(Self { delta })
    }
}

pub(crate) fn required_arg(args: &CommandArgs, key: &str) -> Result<String, CommandError> {
    args.get(key)
        .map(ToOwned::to_owned)
        .ok_or_else(|| CommandError::InvalidArgs(format!("缺少参数：{key}")))
}

pub(crate) fn reject_unknown_args(
    args: &CommandArgs,
    allowed: &[&str],
) -> Result<(), CommandError> {
    if let Some((key, _)) = args.fields().find(|(key, _)| !allowed.contains(key)) {
        return Err(CommandError::InvalidArgs(format!("未知参数：{key}")));
    }
    Ok(())
}

pub(crate) fn parse_optional_bool(value: Option<&str>) -> Result<bool, CommandError> {
    match value {
        None => Ok(false),
        Some("true" | "1" | "yes" | "on") => Ok(true),
        Some("false" | "0" | "no" | "off") => Ok(false),
        Some(other) => Err(CommandError::InvalidArgs(format!("布尔参数非法：{other}"))),
    }
}

pub(crate) fn command_execution_failed(error: EngineError) -> CommandError {
    CommandError::ExecutionFailed(error.to_string())
}

pub(crate) fn format_arg_keys(args: &CommandArgs) -> String {
    args.fields()
        .map(|(key, _)| key)
        .collect::<Vec<_>>()
        .join(", ")
}
