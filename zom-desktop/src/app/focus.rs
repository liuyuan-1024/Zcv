//! 语义焦点管理与 key context 投影。
//!
//! [`App`] 持有唯一的 [`FocusStore`]；本模块的方法负责：
//! - 焦点请求与回退
//! - shell 粗粒度焦点的精化（保留输入态）
//! - 从当前焦点推导 keymap 上下文栈

use zom_command::{FileTreeKeyMode, KeyContext, VersionControlKeyMode};

use crate::focus::{AppFocus, FileTreeFocus, PanelSubFocus, VersionControlFocus};
use crate::ui_id::SurfaceId;

use super::App;

impl App {
    pub(crate) fn request_focus(&mut self, next: AppFocus) {
        self.focus.request(next);
    }

    pub(crate) fn request_focus_from_shell(&mut self, next: AppFocus) {
        let next = self.refine_focus(next);
        self.focus.request(next);
    }

    pub(crate) fn restore_previous_focus(&mut self) -> AppFocus {
        self.focus.restore_previous()
    }

    /// shell 反向同步过来的焦点是粗粒度的（文件树只有 Navigate，不区分 NewEntryName / RenameEntry）。
    ///
    /// 架构保证：`actions.rs` 中 `KeyChord` 只在 GPUI focus 真正切换时才调用本方法，
    /// 不会每次按键都冲刷。因此不再需要「从 `current` 恢复被覆盖焦点」的保留逻辑——
    /// 焦点切换时 `current` 已经是上一个投影目标，不再是 pending 状态。
    ///
    /// 保留两个恢复路径：
    /// - `ConfirmDelete` 是动作确认态，没有对应 TextTargetOwner，无条件保留。
    /// - `RenameEntry` / `NewEntryName` 可能由菜单、命令面板等非键盘路径启动，
    ///   此时 TextTargetOwner 已就绪但 AppFocus 尚未同步，主动检测一次。
    fn refine_focus(&self, focus: AppFocus) -> AppFocus {
        if focus == AppFocus::file_tree(FileTreeFocus::Navigate) {
            // 删除确认不应因焦点投影被吞掉。
            if self.focus.current() == AppFocus::file_tree(FileTreeFocus::ConfirmDelete) {
                return self.focus.current();
            }
            for candidate in [
                AppFocus::file_tree(FileTreeFocus::RenameEntry),
                AppFocus::file_tree(FileTreeFocus::NewEntryName),
            ] {
                if self.text_targets.accepts_focus(&self.session, candidate) {
                    return candidate;
                }
            }
        }
        focus
    }

    /// 把「当前焦点面 + 运行态」映射成 keymap 解析用的 `KeyContext` 优先级栈。
    ///
    /// 这是宿主该做的事 —— 告诉 zom-command「现在处于什么上下文」；
    /// 至于哪个 chord 对应哪条命令，仍由各 catalog 注册进 keymap 的绑定决定。
    pub(super) fn key_contexts(&self) -> Vec<KeyContext> {
        let focus = self.focus.current();
        // 先问 router —— 文本输入类 owner（主编辑区、文件树新建/重命名、搜索框、picker 查询框）
        // 通过 `accepts_focus` 自报家门，由 owner 自己说"我的栈是什么"。
        if let Some(stack) = self.text_targets.key_contexts_for(&self.session, focus) {
            return stack;
        }
        match focus {
            AppFocus::None | AppFocus::Editor(_) => vec![KeyContext::global()],
            AppFocus::SearchBar(_) => {
                unreachable!("SearchModel 是 SearchBar 焦点的 TextTargetOwner，router 必定接管")
            }
            AppFocus::GoToLine => {
                unreachable!("GoToLineModel 是 GoToLine 焦点的 TextTargetOwner，router 必定接管")
            }
            AppFocus::Panel(p) => match p.sub {
                PanelSubFocus::FileTree(FileTreeFocus::ConfirmDelete) => vec![
                    KeyContext::file_tree(FileTreeKeyMode::PendingDelete),
                    KeyContext::global(),
                ],
                PanelSubFocus::FileTree(
                    FileTreeFocus::NewEntryName | FileTreeFocus::RenameEntry,
                ) => vec![KeyContext::global()],
                PanelSubFocus::FileTree(_) => vec![
                    KeyContext::file_tree(FileTreeKeyMode::Navigate),
                    KeyContext::global(),
                ],
                PanelSubFocus::VersionControl(VersionControlFocus::Navigate) => {
                    vec![
                        KeyContext::version_control(VersionControlKeyMode::Navigate),
                        KeyContext::global(),
                    ]
                }
                // CommitMessage 模式由 router 提前接管，此处是兜底安全分支。
                PanelSubFocus::VersionControl(VersionControlFocus::CommitMessage) => {
                    vec![KeyContext::global()]
                }
                PanelSubFocus::Bare => vec![KeyContext::global()],
            },
            AppFocus::Surface(s) => match s.surface {
                SurfaceId::ProjectPicker => vec![KeyContext::global()],
                SurfaceId::Settings => vec![KeyContext::settings(), KeyContext::global()],
                SurfaceId::LanguageServers => {
                    vec![KeyContext::language_servers(), KeyContext::global()]
                }
                SurfaceId::GoToLine => vec![KeyContext::global()],
            },
        }
    }
}
