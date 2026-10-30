//! 项目树的行模型。
//!
//! 项目树实体负责状态和文件操作，本模块只描述一行文件系统条目在树中的投影，供目录遍历、渲染、键盘导航和测试共同消费。

use zcv_git::FileStatus;
use zcv_path::AbsolutePathBuf;
use zcv_ui::TreeRow;

/// 项目树中的一个文件或目录条目。
#[derive(Clone)]
pub(super) struct ProjectTreeRow {
    pub(super) path: AbsolutePathBuf,
    pub(super) name: String,
    pub(super) depth: usize,
    pub(super) is_dir: bool,
    pub(super) expanded: bool,
    pub(super) is_new: bool,
    /// 是否被 Git 忽略。
    /// 与 `git_status` 分离：忽略项不携带 Git 状态色，只按此标志淡显，避免与状态色叠加、也避免依赖状态刷新节奏。
    pub(super) is_ignored: bool,
    /// Git 状态（决定文件名颜色；忽略项恒为 None，None 也表示无状态）。
    pub(super) git_status: Option<FileStatus>,
}

impl TreeRow for ProjectTreeRow {
    fn is_dir(&self) -> bool {
        self.is_dir
    }

    fn depth(&self) -> usize {
        self.depth
    }

    fn expanded(&self) -> bool {
        self.expanded
    }
}
