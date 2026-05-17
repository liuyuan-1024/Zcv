//! app —— 组合根（手册 2 / 13）。
//!
//! 第一版骨架阶段，组合根只持有窗口级布局状态（三个 Dock 与 BottomBar
//! 的少量动态字段），尚未接入 `CommandRegistry` / `Workspace` / `ViewSet` /
//! `Keymap` / `AiProvider` —— 这些进入下一轮再补。
//!
//! 依赖方向（手册 2.4）：`app` 可以 import `shell`；`shell` 不可反向 import `app`。

mod default_layout;

use crate::shell::layout::{BottomBarState, DockState, WorkbenchState};

pub struct App {
    left_dock: DockState,
    right_dock: DockState,
    bottom_dock: DockState,
    bottom_bar: BottomBarState,
}

impl App {
    pub fn new() -> Self {
        Self {
            left_dock: default_layout::default_left_dock(),
            right_dock: default_layout::default_right_dock(),
            bottom_dock: default_layout::default_bottom_dock(),
            bottom_bar: BottomBarState::default(),
        }
    }

    /// 把当前 App 状态投影为 shell 渲染所需的 `WorkbenchState`。
    ///
    /// 骨架阶段直接克隆；将来 dock 状态升级为 `Entity<DockState>` 后，本方法
    /// 改为返回轻量引用 / 句柄包，避免每帧 clone。
    pub(crate) fn workbench_state(&self) -> WorkbenchState {
        WorkbenchState {
            left_dock: self.left_dock.clone(),
            right_dock: self.right_dock.clone(),
            bottom_dock: self.bottom_dock.clone(),
            bottom_bar: self.bottom_bar.clone(),
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
