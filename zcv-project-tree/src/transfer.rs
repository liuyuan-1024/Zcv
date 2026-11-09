//! 文件操作规则：选择净化、移动合法性、目标路径计划与冲突决策会话。
//! 拖拽和粘贴通过同一传输计划进入执行层；交互来源随冲突会话保留。

use std::collections::HashSet;
use zcv_path::AbsolutePathBuf;

/// 净化选中集：排除根行与所有根之外的路径、按路径排序、剔除互为祖先的后代项（目录与其子项同选时只留目录）。
///
/// 路径排序后祖先必先于后代出现，逐项对照已保留前缀即可完成剪枝；
/// `Path::starts_with` 按组件比较，同名前缀（如 `a` 与 `ab`）不会被误判为祖先。
pub(crate) fn sanitize_selection(
    paths: impl IntoIterator<Item = AbsolutePathBuf>,
    roots: &[AbsolutePathBuf],
) -> Vec<AbsolutePathBuf> {
    let mut sorted: Vec<AbsolutePathBuf> = paths
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .filter(|path| !roots.contains(path) && roots.iter().any(|root| path.starts_with(root)))
        .collect();
    sorted.sort();
    let mut kept: Vec<AbsolutePathBuf> = Vec::new();
    for path in sorted {
        if kept.iter().any(|ancestor| path.starts_with(ancestor)) {
            continue;
        }
        kept.push(path);
    }
    kept
}

/// 移动源过滤：拒绝移入自身子树、落回原位置，以及会覆盖源祖先的目标。
/// 多选逐项判断；树内落点预览与最终传输计划共用这条规则。
pub(crate) fn filter_movable_sources(
    sources: &[AbsolutePathBuf],
    target_dir: &AbsolutePathBuf,
) -> Vec<AbsolutePathBuf> {
    sources
        .iter()
        .filter(|source| {
            if target_dir.starts_with(source.as_path()) {
                return false;
            }
            match source.file_name() {
                Some(name) => {
                    let destination = target_dir.join(name);
                    !source.starts_with(&destination)
                }
                None => false,
            }
        })
        .cloned()
        .collect()
}

/// 项目树剪贴板：复制与剪切两种语义（粘贴执行方式由种类决定）。
#[derive(Clone, Debug)]
pub(crate) enum TreeClipboard {
    /// 复制：粘贴为递归复制，源不受影响。
    Copied(Vec<AbsolutePathBuf>),
    /// 剪切：粘贴为移动；首次粘贴后降级为复制。
    Cut(Vec<AbsolutePathBuf>),
}

impl TreeClipboard {
    /// 剪贴板持有路径（复制与剪切共用）。
    pub(crate) fn paths(&self) -> &[AbsolutePathBuf] {
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

/// 传输执行方式：内部剪贴板区分复制与移动，系统文件输入走外部复制。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransferMode {
    Copy,
    CopyExternal,
    Move,
}

/// 文件操作的发起入口；结果呈现与剪贴板生命周期由入口决定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransferOrigin {
    Paste,
    InternalDrag,
    ExternalDrop,
}

/// 输入入口解析出的文件操作请求；执行层只消费确定的源路径和目标目录。
pub(crate) struct TransferRequest {
    pub(crate) mode: TransferMode,
    pub(crate) origin: TransferOrigin,
    pub(crate) target_dir: AbsolutePathBuf,
    pub(crate) sources: Vec<AbsolutePathBuf>,
}

/// 已解析目标路径的传输计划；冲突预检只消费这份确定的项清单。
pub(crate) struct TransferPlan {
    pub(crate) mode: TransferMode,
    pub(crate) origin: TransferOrigin,
    pub(crate) items: Vec<(AbsolutePathBuf, AbsolutePathBuf)>,
}

impl TransferRequest {
    /// 将目标目录与源名称组合为操作清单，并剔除落回原路径的无操作项。
    pub(crate) fn prepare(self) -> TransferPlan {
        let items = self
            .sources
            .into_iter()
            .filter_map(|source| {
                let name = source.file_name()?;
                let dest = AbsolutePathBuf::new(self.target_dir.join(name))
                    .expect("绝对目标目录与源名称组合后仍须为绝对路径");
                (dest != source).then_some((source, dest))
            })
            .collect();
        TransferPlan {
            mode: self.mode,
            origin: self.origin,
            items,
        }
    }
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
    pub(crate) origin: TransferOrigin,
    /// 与冲突项同属本次操作、可直接执行的项。
    pub(crate) clean_items: Vec<(AbsolutePathBuf, AbsolutePathBuf)>,
    /// 冲突项 (源, 目标) 队列。
    pub(crate) items: Vec<(AbsolutePathBuf, AbsolutePathBuf)>,
    /// 已记录的决策（与 items 按序对应）。
    decisions: Vec<ConflictDecision>,
    /// 当前待决策项下标。
    index: usize,
}

impl ConflictSession {
    pub(crate) fn new(
        mode: TransferMode,
        origin: TransferOrigin,
        clean_items: Vec<(AbsolutePathBuf, AbsolutePathBuf)>,
        items: Vec<(AbsolutePathBuf, AbsolutePathBuf)>,
    ) -> Self {
        Self {
            mode,
            origin,
            clean_items,
            items,
            decisions: Vec::new(),
            index: 0,
        }
    }

    /// 当前待决策的冲突项；全部决策完成后为 None。
    pub(crate) fn current_conflict(&self) -> Option<&(AbsolutePathBuf, AbsolutePathBuf)> {
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
pub(crate) fn paste_target_dir(
    selected: Option<&AbsolutePathBuf>,
    selected_is_dir: bool,
) -> Option<AbsolutePathBuf> {
    let path = selected?;
    if selected_is_dir {
        Some(path.clone())
    } else {
        path.parent()
            .and_then(|parent| AbsolutePathBuf::new(parent.to_path_buf()).ok())
    }
}

#[cfg(test)]
#[path = "test/transfer_tests.rs"]
mod tests;
