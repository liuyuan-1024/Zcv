//! 项目树右键菜单：按选中行状态构建菜单项，部署状态由面板持有。
//!
//! 菜单确认经 `ContextMenu` 的焦点上下文回到面板后派发 Action，与键盘快捷键共用面板的既有处理器；关闭路径统一汇聚在部署状态清除。

use gpui::{
    Action, App, ClipboardEntry, Context, DismissEvent, Entity, Focusable, Pixels, Point,
    Subscription, Window,
};
use zcv_actions::{
    TreeCollapse, TreeCopy, TreeCopyAbsolutePath, TreeCopyRelativePath, TreeCut, TreeExpand,
    TreeNewEntry, TreePaste, TreeRemoveFromProject, TreeRename, TreeTrash,
};
use zcv_path::AbsolutePathBuf;
use zcv_ui::{ContextMenu, ContextMenuEntry};

use super::ProjectTreePanel;

/// 构造带快捷键尾注的 action 条目；action 无键位绑定时省略尾注。
fn shortcut_entry(
    label: &str,
    action: impl Action + Clone + 'static,
    cx: &App,
) -> ContextMenuEntry {
    let shortcut = zcv_keymap::display_shortcut(&action, cx);
    let entry = ContextMenuEntry::action(label, Box::new(action));
    match shortcut {
        Some(shortcut) => entry.end_slot_text(shortcut),
        None => entry,
    }
}

/// 已部署的右键菜单；DismissEvent 或点击浮层外部后由面板清除。
pub(super) struct DeployedContextMenu {
    pub(super) menu: Entity<ContextMenu>,
    pub(super) position: Point<Pixels>,
    pub(super) _subscription: Subscription,
}

impl ProjectTreePanel {
    /// 在行上部署右键菜单。
    ///
    /// 右键行不在选中集时收拢为单选，避免菜单操作误伤不可见的旧集合；
    /// 在集合内时保持集合（剪切/复制/删除作用于整个集合），游标移到右键行。
    pub(super) fn deploy_context_menu(
        &mut self,
        path: &AbsolutePathBuf,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 名称编辑或冲突确认进行中不部署：菜单操作会与进行中的会话竞争。
        if self.edit_state.is_some() || self.conflict.is_some() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if state.is_in_selection_set(path) {
                state.move_cursor(path.clone());
            } else {
                state.select(path.clone());
            }
        }
        let menu = self.build_context_menu(window, cx);
        let menu_focus = menu.focus_handle(cx);
        // 先替换面板与订阅，再转移焦点：旧面板失焦时发出的关闭事件因此无人接收，不会误关整个浮层会话。
        let _subscription =
            cx.subscribe_in(&menu, window, |tree, _, _: &DismissEvent, window, cx| {
                tree.dismiss_context_menu(window, cx);
            });
        self.context_menu = Some(DeployedContextMenu {
            menu,
            position,
            _subscription,
        });
        // 菜单开着时右键另一行：会话内换面（焦点域转移，归还目标保持首次打开时的面板）。
        if self.menu_host.is_open() {
            self.menu_host
                .refocus(menu_focus.clone(), menu_focus, window, cx);
        } else {
            let focus_scope = menu_focus.clone();
            self.menu_host.toggle(focus_scope, menu_focus, window, cx);
        }
        cx.notify();
    }

    /// 按选中行状态构建菜单项。
    ///
    /// 根行不可重命名、剪切、复制、删除（与键盘路径的净化规则一致，
    /// 菜单上以禁用态显式表达而不是隐藏）；目录行额外提供展开/折叠，根行额外提供「从项目移除」。
    fn build_context_menu(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<ContextMenu> {
        let (is_root, is_dir) = {
            let state = self.state.borrow();
            state
                .selected_idx()
                .map(|index| {
                    let row = &state.rows()[index];
                    (self.roots.contains(&row.path), row.is_dir)
                })
                .unwrap_or((true, true))
        };
        // 粘贴可用：面板剪贴板或系统剪贴板的外部文件路径任一存在。
        let can_paste = self.clipboard.is_some()
            || cx.read_from_clipboard().is_some_and(|item| {
                item.entries().iter().any(|entry| match entry {
                    ClipboardEntry::ExternalPaths(paths) => !paths.paths().is_empty(),
                    _ => false,
                })
            });
        let focus = self.focus.clone();
        ContextMenu::build(window, cx, move |menu, _, cx| {
            let menu = menu
                .context(focus)
                .entry(shortcut_entry("新建", TreeNewEntry, cx))
                .separator()
                .entry(shortcut_entry("重命名", TreeRename, cx).disabled(is_root))
                .entry(shortcut_entry("剪切", TreeCut, cx).disabled(is_root))
                .entry(shortcut_entry("复制", TreeCopy, cx).disabled(is_root))
                .entry(shortcut_entry("粘贴", TreePaste, cx).disabled(!can_paste))
                .separator()
                .entry(shortcut_entry("复制绝对路径", TreeCopyAbsolutePath, cx).disabled(is_root))
                .entry(shortcut_entry("复制相对路径", TreeCopyRelativePath, cx).disabled(is_root));
            let menu = if is_dir {
                menu.separator()
                    .entry(shortcut_entry("展开", TreeExpand, cx))
                    .entry(shortcut_entry("折叠", TreeCollapse, cx))
            } else {
                menu
            };
            let menu = menu
                .separator()
                .entry(shortcut_entry("删除", TreeTrash, cx).disabled(is_root));
            // 移除只解除 worktree，文件保留在磁盘上；与删除（进回收站）语义不同，分隔呈现。
            if is_root {
                menu.separator()
                    .entry(shortcut_entry("从项目移除", TreeRemoveFromProject, cx))
            } else {
                menu
            }
        })
    }

    /// 关闭已部署的右键菜单并把焦点还给面板；未部署时无操作。
    ///
    /// 焦点归还由浮层会话承担：焦点仍在菜单域时还给打开时记录的面板；
    /// 确认菜单项后焦点已在面板，会话不再抢焦点。
    pub(super) fn dismiss_context_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.context_menu.take().is_some() {
            self.menu_host.dismiss(window, cx);
            cx.notify();
        }
    }
}
