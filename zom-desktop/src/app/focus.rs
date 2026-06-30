//! 语义焦点管理与 key context 投影。
//!
//! [`App`] 持有唯一的 [`FocusStore`]；本模块的方法负责：
//! - 焦点请求与回退
//! - shell 粗粒度焦点的精化（保留输入态）
//! - 从当前焦点推导 keymap 上下文栈

use zom_command::{FileTreeKeyMode, KeyContext};

use crate::focus::{AppFocus, FileTreeFocus, PanelSubFocus};
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
    /// 此方法保留仍有效的输入态——否则 IME commit 会误落到主编辑区。
    fn refine_focus(&self, focus: AppFocus) -> AppFocus {
        if focus == AppFocus::file_tree(FileTreeFocus::Navigate) {
            let current = self.focus.current();
            let current_ft = match current {
                AppFocus::Panel(p) => p.as_file_tree(),
                _ => None,
            };
            if let Some(sub) = current_ft {
                let is_pending = matches!(
                    sub,
                    FileTreeFocus::NewEntryName
                        | FileTreeFocus::RenameEntry
                        | FileTreeFocus::ConfirmDelete,
                );
                if is_pending
                    && (sub == FileTreeFocus::ConfirmDelete
                        || self.text_targets.accepts_focus(&self.session, current))
                {
                    return current;
                }
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
                PanelSubFocus::VersionControl => {
                    vec![KeyContext::version_control(), KeyContext::global()]
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
