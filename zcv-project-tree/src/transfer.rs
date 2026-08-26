//! 传输逻辑纯模块：选中集净化、剪贴板语义、冲突决策会话与粘贴目标推断。
//!
//! copy/cut/paste 的语义规则集中于此（面板 mod.rs 只做接线），便于单测覆盖；
//! `sanitize_selection` 由面板 mod.rs 迁入，原实现与单测一并迁移。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// 净化选中集：排除根行与项目外路径、按路径排序、剔除互为祖先的后代项（目录与其子项同选时只留目录）。
///
/// 路径排序后祖先必先于后代出现，逐项对照已保留前缀即可完成剪枝；
/// `Path::starts_with` 按组件比较，同名前缀（如 `a` 与 `ab`）不会被误判为祖先。
pub(crate) fn sanitize_selection(
    paths: impl IntoIterator<Item = PathBuf>,
    root: &Path,
) -> Vec<PathBuf> {
    let mut sorted: Vec<PathBuf> = paths
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .filter(|path| path != root && path.starts_with(root))
        .collect();
    sorted.sort();
    let mut kept: Vec<PathBuf> = Vec::new();
    for path in sorted {
        if kept.iter().any(|ancestor| path.starts_with(ancestor)) {
            continue;
        }
        kept.push(path);
    }
    kept
}

/// 项目树剪贴板：复制与剪切两种语义（粘贴执行方式由种类决定）。
#[derive(Clone, Debug)]
pub(crate) enum TreeClipboard {
    /// 复制：粘贴为递归复制，源不受影响。
    Copied(Vec<PathBuf>),
    /// 剪切：粘贴为移动；首次粘贴后降级为复制（对齐 Zed）。
    Cut(Vec<PathBuf>),
}

impl TreeClipboard {
    /// 剪贴板持有路径（复制与剪切共用）。
    pub(crate) fn paths(&self) -> &[PathBuf] {
        match self {
            Self::Copied(paths) | Self::Cut(paths) => paths,
        }
    }

    /// 剪切降级为复制：首次粘贴完成后调用，剪切项仍可再次粘贴。
    /// 已是复制时原样返回。
    pub(crate) fn into_copied(self) -> Self {
        match self {
            Self::Cut(paths) => Self::Copied(paths),
            copied @ Self::Copied(_) => copied,
        }
    }
}

/// 传输执行方式：由剪贴板种类决定（Cut 粘贴全为 Move、Copied 粘贴全为 Copy）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransferMode {
    Copy,
    Move,
}

/// 冲突项的用户决策。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConflictDecision {
    /// 覆盖已存在的目标。
    Overwrite,
    /// 跳过该项（源与目标都不动）。
    Skip,
}

/// 冲突确认会话：逐项收集「目标已存在」项的决策，全部决策完成后统一执行。
///
/// items 为 (源绝对路径, 目标绝对路径) 对（仅含冲突项，构造时全部预检好）；
/// 非冲突项不入会话队列，由宿主在会话结束后与冲突项合成完整执行清单。
#[derive(Debug)]
pub(crate) struct ConflictSession {
    /// 传输方式（Copy/Move），决策完成后据此分派执行。
    pub(crate) mode: TransferMode,
    /// 粘贴目标目录（浮层文案与执行期参考）。
    pub(crate) target_dir: PathBuf,
    /// 冲突项 (源, 目标) 队列。
    pub(crate) items: Vec<(PathBuf, PathBuf)>,
    /// 已记录的决策（与 items 按序对应）。
    decisions: Vec<ConflictDecision>,
    /// 当前待决策项下标。
    index: usize,
}

impl ConflictSession {
    pub(crate) fn new(
        mode: TransferMode,
        target_dir: PathBuf,
        items: Vec<(PathBuf, PathBuf)>,
    ) -> Self {
        Self {
            mode,
            target_dir,
            items,
            decisions: Vec::new(),
            index: 0,
        }
    }

    /// 当前待决策的冲突项；全部决策完成后为 None。
    pub(crate) fn current_conflict(&self) -> Option<&(PathBuf, PathBuf)> {
        self.items.get(self.index)
    }

    /// 记录当前项的决策并推进到下一项。
    pub(crate) fn record_decision(&mut self, decision: ConflictDecision) {
        self.decisions.push(decision);
        self.index += 1;
    }

    /// 是否全部决策完成（会话可出队执行）。
    pub(crate) fn is_resolved(&self) -> bool {
        self.decisions.len() == self.items.len()
    }

    /// 会话是否为空（无冲突项）。
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 冲突项数量。
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// 已记录的决策序列（与冲突项按序对应）。
    pub(crate) fn decisions(&self) -> &[ConflictDecision] {
        &self.decisions
    }
}

/// 粘贴目标目录推断：选中目录→自身；选中文件→父目录；无选中→None。
///
/// 面板从行模型解析游标行的 `is_dir` 后以布尔传入（最简签名，不引入回调）。
pub(crate) fn paste_target_dir(selected: Option<&Path>, selected_is_dir: bool) -> Option<PathBuf> {
    let path = selected?;
    if selected_is_dir {
        Some(path.to_path_buf())
    } else {
        path.parent().map(Path::to_path_buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_selection_drops_descendants_sorts_and_excludes_root() {
        let root = Path::new("/proj");
        let dir = root.join("src");
        let file = dir.join("main.rs");
        let sibling = root.join("a.txt");
        // 乱序输入：目录与其子文件同选只留目录，根被剔除，输出按路径排序。
        assert_eq!(
            sanitize_selection(
                [
                    file.clone(),
                    sibling.clone(),
                    dir.clone(),
                    root.to_path_buf()
                ],
                root
            ),
            vec![sibling, dir]
        );
    }

    #[test]
    fn sanitize_selection_excludes_paths_outside_project() {
        let root = Path::new("/proj");
        let outside = PathBuf::from("/other/file.txt");
        let inside = root.join("b.txt");
        assert_eq!(
            sanitize_selection([root.to_path_buf(), outside, inside.clone()], root),
            vec![inside]
        );
    }

    #[test]
    fn sanitize_selection_treats_sibling_prefix_as_non_ancestor() {
        let root = Path::new("/proj");
        let a = root.join("a");
        let ab = root.join("ab");
        // 组件级比较：a 与 ab 互不为祖先，两条都保留。
        assert_eq!(
            sanitize_selection([ab.clone(), a.clone()], root),
            vec![a, ab]
        );
    }

    #[test]
    fn cut_clipboard_degrades_to_copied_after_paste() {
        let paths = vec![PathBuf::from("/proj/a.txt")];
        let clipboard = TreeClipboard::Cut(paths.clone()).into_copied();
        assert!(matches!(clipboard, TreeClipboard::Copied(ref degraded) if degraded == &paths));
        // 已是复制：原样返回，不改动。
        let copied = TreeClipboard::Copied(paths.clone()).into_copied();
        assert!(matches!(copied, TreeClipboard::Copied(ref kept) if kept == &paths));
    }

    #[test]
    fn clipboard_paths_accessor_covers_both_variants() {
        let paths = vec![PathBuf::from("/proj/a.txt")];
        assert_eq!(
            TreeClipboard::Copied(paths.clone()).paths(),
            paths.as_slice()
        );
        assert_eq!(TreeClipboard::Cut(paths.clone()).paths(), paths.as_slice());
    }

    /// 构造三项冲突会话：源/目标对按序可分辨。
    fn session(mode: TransferMode) -> ConflictSession {
        ConflictSession::new(
            mode,
            PathBuf::from("/proj/dst"),
            vec![
                (
                    PathBuf::from("/proj/a.txt"),
                    PathBuf::from("/proj/dst/a.txt"),
                ),
                (
                    PathBuf::from("/proj/b.txt"),
                    PathBuf::from("/proj/dst/b.txt"),
                ),
                (
                    PathBuf::from("/proj/c.txt"),
                    PathBuf::from("/proj/dst/c.txt"),
                ),
            ],
        )
    }

    #[test]
    fn session_with_all_overwrite_decisions_resolves_in_order() {
        let mut session = session(TransferMode::Copy);
        assert_eq!(session.len(), 3);
        assert!(!session.is_empty());
        assert_eq!(
            session.current_conflict().map(|(source, _)| source),
            Some(&PathBuf::from("/proj/a.txt"))
        );

        session.record_decision(ConflictDecision::Overwrite);
        assert_eq!(
            session.current_conflict().map(|(source, _)| source),
            Some(&PathBuf::from("/proj/b.txt"))
        );
        assert!(!session.is_resolved());

        session.record_decision(ConflictDecision::Overwrite);
        session.record_decision(ConflictDecision::Overwrite);
        assert!(session.is_resolved());
        assert_eq!(session.current_conflict(), None);
        assert_eq!(
            session.decisions(),
            &[ConflictDecision::Overwrite; 3],
            "全部覆盖：三项决策按序记录"
        );
    }

    #[test]
    fn session_with_all_skip_decisions_resolves() {
        let mut session = session(TransferMode::Move);
        for _ in 0..3 {
            session.record_decision(ConflictDecision::Skip);
        }
        assert!(session.is_resolved());
        assert_eq!(session.decisions(), &[ConflictDecision::Skip; 3]);
    }

    #[test]
    fn session_with_mixed_decisions_preserves_order() {
        let mut session = session(TransferMode::Copy);
        session.record_decision(ConflictDecision::Overwrite);
        session.record_decision(ConflictDecision::Skip);
        session.record_decision(ConflictDecision::Overwrite);
        assert!(session.is_resolved());
        assert_eq!(
            session.decisions(),
            &[
                ConflictDecision::Overwrite,
                ConflictDecision::Skip,
                ConflictDecision::Overwrite
            ]
        );
    }

    #[test]
    fn empty_session_is_immediately_resolved() {
        let session =
            ConflictSession::new(TransferMode::Copy, PathBuf::from("/proj/dst"), Vec::new());
        assert!(session.is_empty());
        assert_eq!(session.len(), 0);
        assert!(session.is_resolved(), "空会话无待决策项，视为已解决");
        assert_eq!(session.current_conflict(), None);
    }

    #[test]
    fn paste_target_dir_follows_selection_kind() {
        let root = Path::new("/proj");
        // 目录 → 自身。
        assert_eq!(
            paste_target_dir(Some(&root.join("src")), true),
            Some(root.join("src"))
        );
        // 文件 → 父目录。
        assert_eq!(
            paste_target_dir(Some(&root.join("src").join("main.rs")), false),
            Some(root.join("src"))
        );
        // 根级文件的父目录即项目根。
        assert_eq!(
            paste_target_dir(Some(&root.join("a.txt")), false),
            Some(root.to_path_buf())
        );
        // 无选中 → None。
        assert_eq!(paste_target_dir(None, true), None);
    }
}
