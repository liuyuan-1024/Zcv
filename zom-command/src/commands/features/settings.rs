//! `settings.*` 命令目录。
//!
//! 设置界面暂未实现；命令先完整注册，宿主收到 effect 后决定展示占位或忽略。
//!
//! esc 走系统级 [`crate::commands::system::dismiss::DISMISS_TOP`]（scope=Settings）—— [`OPEN`] 推一条 dismiss token，esc 弹出后重新派发 [`DISMISS`]。

use crate::commands::cid;
use crate::commands::system::dismiss as dismiss_top;
use crate::{
    CommandArgs, CommandContext, CommandError, CommandHandler, CommandOutcome, CommandRegistry,
    DismissScope, HostEffect, Invocation, KeyBindingContext, Keymap, NoArgs, SettingsChangeRequest,
    SurfaceEffect, reject_unknown_args, required_arg,
};

/// 打开设置面板。
pub const OPEN: &str = "settings.open";
/// 关闭设置面板。
pub const DISMISS: &str = "settings.dismiss";
/// 打开真实的 config.toml。
pub const OPEN_TOML: &str = "settings.open_toml";
/// 应用一项设置变更。
pub const APPLY_CHANGE: &str = "settings.apply_change";
/// 增大编辑器字号。
pub const INCREASE_EDITOR_FONT_SIZE: &str = "settings.increase_editor_font_size";
/// 减小编辑器字号。
pub const DECREASE_EDITOR_FONT_SIZE: &str = "settings.decrease_editor_font_size";
/// 增大UI字号。
pub const INCREASE_UI_FONT_SIZE: &str = "settings.increase_ui_font_size";
/// 减小UI字号。
pub const DECREASE_UI_FONT_SIZE: &str = "settings.decrease_ui_font_size";

/// 设置面板拥有自己的键盘上下文，Esc 等面板内按键不污染全局快捷键空间。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SettingsKeyContext;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SettingsBindingContext;

pub fn open() -> Invocation {
    (cid(OPEN), CommandArgs::new())
}

pub fn dismiss() -> Invocation {
    (cid(DISMISS), CommandArgs::new())
}

pub fn open_toml() -> Invocation {
    (cid(OPEN_TOML), CommandArgs::new())
}

pub fn apply_change(change: SettingsChangeRequest) -> Invocation {
    (cid(APPLY_CHANGE), SettingsChangeArgs { change }.into())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SettingsChangeArgs {
    pub change: SettingsChangeRequest,
}

impl From<SettingsChangeArgs> for CommandArgs {
    fn from(args: SettingsChangeArgs) -> Self {
        match args.change {
            SettingsChangeRequest::AdjustUiFont(delta) => CommandArgs::new()
                .with("kind", "adjust_ui_font")
                .with("delta", delta.to_string()),
            SettingsChangeRequest::AdjustEditorFont(delta) => CommandArgs::new()
                .with("kind", "adjust_editor_font")
                .with("delta", delta.to_string()),
            SettingsChangeRequest::ToggleEditorSoftWrap => {
                CommandArgs::new().with("kind", "toggle_editor_soft_wrap")
            }
            SettingsChangeRequest::CycleEditorTabSize => {
                CommandArgs::new().with("kind", "cycle_editor_tab_size")
            }
            SettingsChangeRequest::CycleTheme => CommandArgs::new().with("kind", "cycle_theme"),
        }
    }
}

impl TryFrom<CommandArgs> for SettingsChangeArgs {
    type Error = CommandError;

    fn try_from(args: CommandArgs) -> Result<Self, Self::Error> {
        reject_unknown_args(&args, &["kind", "delta"])?;
        let kind = required_arg(&args, "kind")?;
        let change = match kind.as_str() {
            "adjust_ui_font" => SettingsChangeRequest::AdjustUiFont(required_delta(&args)?),
            "adjust_editor_font" => SettingsChangeRequest::AdjustEditorFont(required_delta(&args)?),
            "toggle_editor_soft_wrap" => SettingsChangeRequest::ToggleEditorSoftWrap,
            "cycle_editor_tab_size" => SettingsChangeRequest::CycleEditorTabSize,
            "cycle_theme" => SettingsChangeRequest::CycleTheme,
            other => {
                return Err(CommandError::InvalidArgs(format!(
                    "未知设置变更类型：{other}"
                )));
            }
        };
        Ok(Self { change })
    }
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    let settings = KeyBindingContext::settings();

    registry
        .install(keymap, OPEN, "设置", Box::new(run_open))
        .description("打开设置面板。")
        .key("mod ,");
    registry.install(keymap, DISMISS, "关闭设置", Box::new(run_dismiss));
    dismiss_top::bind_esc(keymap, DismissScope::Settings, settings);

    registry
        .install(keymap, OPEN_TOML, "打开设置 TOML", Box::new(run_open_toml))
        .hide_from_shortcuts();
    registry
        .install(
            keymap,
            APPLY_CHANGE,
            "应用设置变更",
            Box::new(run_apply_change),
        )
        .hide_from_shortcuts();

    registry
        .install(
            keymap,
            INCREASE_EDITOR_FONT_SIZE,
            "增大编辑器字号",
            run_adjust_font_size(SettingsChangeRequest::AdjustEditorFont(1)),
        )
        .description("增大编辑器字号。")
        .key("mod =");
    registry
        .install(
            keymap,
            DECREASE_EDITOR_FONT_SIZE,
            "减小编辑器字号",
            run_adjust_font_size(SettingsChangeRequest::AdjustEditorFont(-1)),
        )
        .description("减小编辑器字号。")
        .key("mod -");
    registry
        .install(
            keymap,
            INCREASE_UI_FONT_SIZE,
            "增大UI字号",
            run_adjust_font_size(SettingsChangeRequest::AdjustUiFont(1)),
        )
        .description("增大UI字号。")
        .key("mod +");
    registry
        .install(
            keymap,
            DECREASE_UI_FONT_SIZE,
            "减小UI字号",
            run_adjust_font_size(SettingsChangeRequest::AdjustUiFont(-1)),
        )
        .description("减小UI字号。")
        .key("mod _");
}

fn run_open(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.dismiss.clear(DismissScope::Settings);
    context
        .dismiss
        .push(DismissScope::Settings, "关闭设置", dismiss());
    context
        .effects
        .push(HostEffect::Surface(SurfaceEffect::ShowSettings));
    Ok(CommandOutcome::default())
}

fn run_dismiss(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context.dismiss.clear(DismissScope::Settings);
    context
        .effects
        .push(HostEffect::Surface(SurfaceEffect::Dismiss));
    Ok(CommandOutcome::default())
}

fn run_open_toml(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    NoArgs::try_from(args)?;
    context
        .effects
        .push(HostEffect::Surface(SurfaceEffect::OpenSettingsToml));
    Ok(CommandOutcome::default())
}

fn run_apply_change(
    context: &mut CommandContext<'_>,
    args: CommandArgs,
) -> Result<CommandOutcome, CommandError> {
    let args = SettingsChangeArgs::try_from(args)?;
    context
        .effects
        .push(HostEffect::Surface(SurfaceEffect::ApplySettingsChange(
            args.change,
        )));
    Ok(CommandOutcome::default())
}

fn run_adjust_font_size(change: SettingsChangeRequest) -> CommandHandler {
    Box::new(move |context, _args| {
        context
            .effects
            .push(HostEffect::Surface(SurfaceEffect::ApplySettingsChange(
                change,
            )));
        Ok(CommandOutcome::default())
    })
}

fn required_delta(args: &CommandArgs) -> Result<i16, CommandError> {
    let raw = required_arg(args, "delta")?;
    raw.parse()
        .map_err(|_| CommandError::InvalidArgs(format!("无效设置变更步长：{raw}")))
}
