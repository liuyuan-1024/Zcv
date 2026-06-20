//! Shell → App 反向接入端口。
//!
//! 顶层位置（而非藏在 `app/`）是有意的：trait 的实现方在 shell（feature runtime 自报家门），调用方在 app（BackgroundPumps 按注册顺序回调）。
//! 把它放在 `shell` 与 `app` 都能 import 的公共词汇表里，比让 shell `use crate::app::pumps::*` 在概念上更准确——后者会让"shell 依赖 app" 的红线没法用 import 路径检查。
//!
//! 新增端口的规则：trait 只描述"shell 实现什么"，**不要**让它持有 `App` / `ShellHost` 引用；
//! 状态由实现方自己用 [`Rc<RefCell<...>>`] 等内部可变容器维持。

use zom_command::{BubbleRequest, SearchOption};

use crate::workspace_session::WorkspaceSession;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SearchAction {
    /// 搜索栏刚被打开 —— 立刻把 query 推进活动 buffer 的 BufferSearch 并 reveal 首条命中。
    Opened,
    /// 搜索栏关闭 —— 清掉活动 buffer 的命中高亮，标记 open=false 阻止后续 sync 复活。
    Closed,
    ToggleOption(SearchOption),
    FindPrevious,
    FindNext,
    ReplaceNext,
    ReplaceAll,
    /// 把活动 view 的光标折叠到当前命中末尾。
    ConfirmMatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileTreeAction {
    Activate,
    CommitNewEntry,
    CommitRename,
    ConfirmDelete,
    Paste,
}

#[derive(Default)]
pub(crate) struct FileTreeActionResult {
    pub(crate) opened_file: bool,
    pub(crate) bubbles: Vec<BubbleRequest>,
}

/// 文件树会话动作端口：shell feature 拥有文件树模型，
/// app 只在动作需要 [`WorkspaceSession`] 时带着会话调这个端口，避免 shell 直接借出 app 内部 session。
pub(crate) trait FileTreeHost {
    fn apply_file_tree_action_from_effect(
        &self,
        action: FileTreeAction,
        session: &mut WorkspaceSession,
    ) -> FileTreeActionResult;
}

/// 搜索动作端口：search feature 拥有 query/replacement/options，app 负责把
/// 这些动作和 workspace/view 会话组合在一起执行。
pub(crate) trait SearchHost {
    fn apply_search_action_from_effect(&self, action: SearchAction, session: &mut WorkspaceSession);
    fn is_open(&self) -> bool;
}

/// 编辑后同步端口：每次活动 buffer 上产生编辑事件后被调一次。
///
/// 顺序保证：built-in 的活动 buffer post_edit 先跑（把 buffer 上的 DeltaEvent 扇出给搜索 / 语法 provider），然后才轮到注册的观察者
/// ——后者通常依赖前者已经把事件推到自己关心的状态机里。
pub(crate) trait PostEditObserver {
    fn after_text_edit(&self, session: &mut WorkspaceSession);
}

/// 每帧端口：每帧 prepaint 起手按注册顺序被调一次。
///
/// 不保证调用线程之外的协调；实现内部如果有 RefCell，自己负责借用周期。
pub(crate) trait FramePump {
    fn pump(&self, session: &mut WorkspaceSession);
}
