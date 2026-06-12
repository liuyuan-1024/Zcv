//! 宿主意图：shell / editor 共享的离散用户动作派发词汇。

use std::rc::Rc;

use gpui::{App as GpuiApp, Window};
use zom_command::Invocation;
use zom_command::commands::editor as editor_commands;
use zom_command::commands::editor::ImeUtf16RangeArgs;
use zom_engine::ByteOffset;

use crate::editor::text::ImeUtf16Range;
use crate::focus::AppFocus;

/// shell 预绑定的命令意图窄口。
///
/// UI 子组件只拿到这个请求回调，不接触命令 id、Invocation 或 registry。
/// 触发后进入 [`HostIntent::Command`]，再由统一 host intent 管线派发。
pub(crate) type CommandRequest = Rc<dyn Fn(&mut Window, &mut GpuiApp)>;

/// shell 预绑定的按键意图窄口。
///
/// 返回 `true` 表示按键被 keymap 消费，调用方应当停止传播；
/// 返回 `false` 表示没有匹配，必须放行给系统输入法。
pub(crate) type KeyRequest = Rc<dyn Fn(String, &mut Window, &mut GpuiApp) -> bool>;

/// shell / editor 之间的统一宿主意图出口。
pub(crate) type HostIntentRequest =
    Rc<dyn Fn(HostIntent, &mut Window, &mut GpuiApp) -> HostIntentOutcome>;

/// 一次宿主意图派发的结果。
pub(crate) struct HostIntentOutcome {
    pub(crate) consumed: bool,
}

impl HostIntentOutcome {
    pub(crate) fn consumed() -> Self {
        Self { consumed: true }
    }

    pub(crate) fn passed_through() -> Self {
        Self { consumed: false }
    }
}

/// 宿主意图。
pub(crate) enum HostIntent {
    /// 明确要执行哪条命令
    Command(Invocation),
    /// 键盘快捷键输入
    KeyChord(String),
    /// 系统输入法文本输入
    Ime(ImeIntent),
    /// 设备连续交互。
    Interaction(InteractionIntent),
}

/// 设备连续交互的领域意图。
pub(crate) enum InteractionIntent {
    Pointer(PointerIntent),
}

/// 鼠标 / 触控板交互意图。
pub(crate) enum PointerIntent {
    SetSelection {
        focus: AppFocus,
        anchor: ByteOffset,
        head: ByteOffset,
    },
    ScrollViewport {
        focus: AppFocus,
        delta_visual_rows: i64,
    },
}

/// 系统输入法写入路径的领域意图。
pub(crate) enum ImeIntent {
    Confirm,
    Commit {
        range: Option<ImeUtf16Range>,
        text: String,
    },
    Update {
        range: Option<ImeUtf16Range>,
        text: String,
        selected_range: Option<ImeUtf16Range>,
    },
}

impl ImeIntent {
    pub(crate) fn into_invocation(self) -> Invocation {
        match self {
            Self::Confirm => editor_commands::ime_confirm(),
            Self::Commit { range, text } => {
                editor_commands::ime_commit(range.map(ime_range_args), text)
            }
            Self::Update {
                range,
                text,
                selected_range,
            } => editor_commands::ime_update(
                range.map(ime_range_args),
                text,
                selected_range.map(ime_range_args),
            ),
        }
    }
}

fn ime_range_args(range: ImeUtf16Range) -> ImeUtf16RangeArgs {
    ImeUtf16RangeArgs::new(range.start(), range.end()).expect("IME range 已在边界层校验")
}
