//! 默认 Dock 映射 —— 第一版固定布局（按 BottomBar 分组对应停靠区）。
//!
//! - LeftDock  ← 文件树 / 版本管理 / 大纲 / 项目搜索（默认 active 文件树）
//! - BottomDock ← 终端 / 调试（默认 active 终端）
//! - RightDock ← 快捷键
//!
//! 同一时间一个 Dock 内最多显示 1 个 panel（手册 20.4 单栈模型）。
//! `settings` 可覆盖 panel 顺序与初始 active，但不可改 panel 归属 dock（第一版）。

use gpui::px;

use crate::shell::model::{DockState, PanelId, PanelStack};

pub(super) fn default_left_dock() -> DockState {
    DockState {
        collapsed: false,
        size: px(240.0),
        stack: PanelStack::new(
            vec![
                PanelId::FileTree,
                PanelId::VersionControl,
                PanelId::Outline,
                PanelId::ProjectSearch,
            ],
            Some(PanelId::FileTree),
        ),
    }
}

pub(super) fn default_right_dock() -> DockState {
    DockState {
        collapsed: false,
        size: px(240.0),
        stack: PanelStack::new(
            vec![PanelId::KeyboardShortcuts],
            Some(PanelId::KeyboardShortcuts),
        ),
    }
}

pub(super) fn default_bottom_dock() -> DockState {
    DockState {
        collapsed: false,
        size: px(200.0),
        stack: PanelStack::new(
            vec![PanelId::Terminal, PanelId::Debug],
            Some(PanelId::Terminal),
        ),
    }
}
