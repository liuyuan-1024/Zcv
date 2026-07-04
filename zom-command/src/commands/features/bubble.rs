//! 气泡命令目录：复制文本等。
//!
//! 气泡不属于编辑器，但它需要走命令管线来写剪贴板。
//! 这里只声明 id、builder 和 handler，handler 直接借用 `CommandContext::clipboard` 的 `ClipboardPort` 写入，不依赖编辑器选区。

use crate::commands::cid;
use crate::{
    CommandArgs, CommandContext, CommandError, CommandOutcome, CommandRegistry, Invocation, Keymap,
    reject_unknown_args, required_arg,
};

pub const COPY: &str = "bubble.copy";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CopyArgs {
    pub text: String,
}

impl CopyArgs {
    const KEY_TEXT: &str = "text";
}

impl From<CopyArgs> for CommandArgs {
    fn from(args: CopyArgs) -> Self {
        CommandArgs::new().with(CopyArgs::KEY_TEXT, args.text)
    }
}

impl TryFrom<CommandArgs> for CopyArgs {
    type Error = CommandError;
    fn try_from(args: CommandArgs) -> Result<Self, Self::Error> {
        reject_unknown_args(&args, &[CopyArgs::KEY_TEXT])?;
        Ok(Self {
            text: required_arg(&args, CopyArgs::KEY_TEXT)?,
        })
    }
}

pub fn copy(text: impl Into<String>) -> Invocation {
    (cid(COPY), CopyArgs { text: text.into() }.into())
}

fn run_copy(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = CopyArgs::try_from(args)?;
    context.clipboard.write(&args.text);
    Ok(CommandOutcome::default())
}

/// 注册 bubble 域命令。当前只有 `bubble.copy`，只供程序化调用，不绑快捷键。
pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    registry.install(keymap, COPY, "复制", Box::new(run_copy));
}
