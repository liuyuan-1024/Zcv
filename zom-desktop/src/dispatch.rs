//! app 与 shell 共享的派发结果词汇。

use zom_command::HostEffect;

/// 一次按键派发的结果。
///
/// `consumed=false` 表示这次按键没有匹配任何 keymap 绑定，应当透传给系统输入法；
/// 否则会阻塞 IME 的整个文本输入路径。
pub(crate) struct KeyDispatchOutcome {
    pub(crate) consumed: bool,
    pub(crate) effects: Vec<HostEffect>,
}
