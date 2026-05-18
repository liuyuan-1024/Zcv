//! 命令派发、keymap 解析与 `HostEffect` 翻译。

use crate::app::App;
use crate::shell::model::{DockState, PanelId};
use crate::shell::platform::window::WindowAction;

use zom_command::{
    CommandArgs, CommandContext, CommandError, CommandId, EffectQueue, HostEffect, Invocation,
    KeyChord, KeymapResolution,
};

/// 一次按键派发的结果。`consumed=false` 表示这次按键没有匹配任何 keymap
/// 绑定，应当**透传给系统输入法**；否则会阻塞 IME 的整个文本输入路径。
pub(crate) struct KeyDispatchOutcome {
    pub(crate) consumed: bool,
    pub(crate) actions: Vec<WindowAction>,
}

impl App {
    /// 派发一次命令调用。
    ///
    /// 调用方应当来自 typed builder（如 `editor::insert_text("hi")` /
    /// `commands::window::quit()`），从而避免在调用点手写 `CommandId::new(...)`
    /// 或 `CommandArgs::new().with(...)` —— 字符串字面量收拢在 catalog 模块内。
    pub(crate) fn dispatch(
        &mut self,
        invocation: Invocation,
    ) -> Result<Vec<WindowAction>, CommandError> {
        let (id, args) = invocation;
        self.dispatch_command_id(id, args)
    }

    /// 处理一次归一化按键。
    ///
    /// 文本输入（普通可打印字符、IME 提交）不在这里 fallback：交给 GPUI 的
    /// `EntityInputHandler` 路径，由系统输入法或 NSTextInputClient 把文本喂给
    /// `App::ime_*`。这里只负责走 keymap → 命令。
    ///
    /// 返回是否消费了这次按键 —— 调用方（shell 的 on_key_down）只有在被消费时
    /// 才能 `stop_propagation`，否则 macOS 会把这一帧的按键标记为已处理，
    /// NSTextInputClient 永远拿不到，输入法直接哑掉。
    pub(crate) fn dispatch_key_input(
        &mut self,
        chord: String,
    ) -> Result<KeyDispatchOutcome, CommandError> {
        let chord = KeyChord::new(chord)?;
        match self.keymap.resolve(&[chord], &[]) {
            KeymapResolution::Matched { command, args } => {
                let actions = self.dispatch_command_id(command, args)?;
                Ok(KeyDispatchOutcome {
                    consumed: true,
                    actions,
                })
            }
            // 多段 leader key 待续：吃掉这一击，等下一击。
            KeymapResolution::Pending => Ok(KeyDispatchOutcome {
                consumed: true,
                actions: Vec::new(),
            }),
            // 没有任何绑定：把按键留给系统输入法 / NSTextInputClient。
            KeymapResolution::NoMatch => Ok(KeyDispatchOutcome {
                consumed: false,
                actions: Vec::new(),
            }),
        }
    }

    /// 查询某条命令的快捷键文案 —— 给 Glyph / 命令面板 / 菜单用。
    ///
    /// 反查 + 格式化都在 [`zom_command::Keymap::format_shortcut_for`] 里，UI
    /// 拿到的就是当前平台合适的字符串；本方法只做 `&str` → `CommandId` 的
    /// 包装。找不到绑定返回 `None`，UI 应当表现为“无快捷键”。
    pub(crate) fn shortcut_for(&self, command_id: &str) -> Option<String> {
        let command = CommandId::new(command_id).ok()?;
        self.keymap.format_shortcut_for(&command)
    }

    fn dispatch_command_id(
        &mut self,
        id: CommandId,
        args: CommandArgs,
    ) -> Result<Vec<WindowAction>, CommandError> {
        self.queue.dispatch(id, args);

        // 每次派发用一份新的 effect 队列，命令产生的副作用不会跨派发泄漏。
        let mut effects = EffectQueue::new();
        let mut context = CommandContext {
            workspace: &mut self.workspace,
            views: &mut self.views,
            queue: &mut self.queue,
            effects: &mut effects,
        };
        let result = self.executor.run(&self.registry, &mut context);

        // 翻译 effect：window 类返回给上层（ShellView 才能调 GPUI），
        // dock 类直接在 App 内 apply。
        let mut window_actions: Vec<WindowAction> = Vec::new();
        for effect in effects.drain() {
            self.apply_host_effect(effect, &mut window_actions);
        }

        result?;
        Ok(window_actions)
    }

    /// 翻译命令系统反馈的 [`HostEffect`] 到具体宿主动作。
    ///
    /// 加新 effect 变体时必须在此 match 补分支 —— 编译期就能发现遗漏，
    /// 这是闭合枚举的好处。
    fn apply_host_effect(&mut self, effect: HostEffect, window_actions: &mut Vec<WindowAction>) {
        match effect {
            HostEffect::Quit => window_actions.push(WindowAction::Quit),
            HostEffect::Minimize => window_actions.push(WindowAction::Minimize),
            HostEffect::ToggleMaximize => window_actions.push(WindowAction::ToggleMaximize),
            HostEffect::TogglePanel(panel_str_id) => {
                let Some(panel) = PanelId::from_command_str_id(&panel_str_id) else {
                    eprintln!("HostEffect::TogglePanel 收到未知 panel id：{panel_str_id}");
                    return;
                };
                self.toggle_panel(panel);
            }
        }
    }

    fn toggle_panel(&mut self, panel: PanelId) {
        let Some(dock) = self.dock_hosting_mut(panel) else {
            return;
        };
        if dock.stack.active() == Some(panel) && !dock.collapsed {
            dock.collapsed = true;
        } else {
            dock.collapsed = false;
            dock.stack.active = Some(panel);
        }
    }

    fn dock_hosting_mut(&mut self, panel: PanelId) -> Option<&mut DockState> {
        for dock in [
            &mut self.left_dock,
            &mut self.right_dock,
            &mut self.bottom_dock,
        ] {
            if dock.stack.contains(panel) {
                return Some(dock);
            }
        }
        None
    }
}
