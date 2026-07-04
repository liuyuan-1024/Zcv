//! 应用层焦点模型 —— 唯一语义真相源。
//!
//! `AppFocus` 是命令分发、快捷键 context、IME 路由、状态栏显示的真相源。
//! Panel / Surface 的"身份"与"焦点"共用同一组枚举：[`PanelId`] / [`SurfaceId`]，
//! 焦点侧只在它们之上挂一个可选 sub-focus，避免两套同名变体并行漂移。
//!
//! GPUI 的 [`FocusHandle`](gpui::FocusHandle) 是它在窗口系统里的投影，对应桥接层在 [`crate::shell::view::focus`]；
//! 本模块不依赖 GPUI，也不引用 shell 内部，只复用 crate 顶层的 [`crate::ui_id`] 纯数据枚举。

use crate::ui_id::{PanelId, SurfaceId};

/// 应用的唯一语义焦点：当前用户操作应该落到哪里。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(crate) enum AppFocus {
    /// 没有任何已知业务目标获得焦点，只放行全局快捷键。
    #[default]
    None,
    /// 主编辑区。具体 view 由 `ViewSet::active()` 当场决定，FocusStore 不缓存。
    Editor(EditorFocus),
    /// Workbench panel。
    Panel(PanelFocus),
    /// 浮面 / palette 类 surface。
    Surface(SurfaceFocus),
    /// 编辑器上方的内联搜索栏的某一个输入框。
    /// 搜索栏不挂在 dock 也不是 surface，单独立一类。
    SearchBar(SearchField),
    /// 跳转到指定行列的输入栏。
    GoToLine,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum EditorFocus {
    Main,
}

/// Panel 焦点：哪一个 panel（位置维度）+ 该 panel 内的 sub-focus（子模式维度）。
///
/// 字段公开，但**只能**通过 [`PanelFocus::bare`] / [`PanelFocus::file_tree`] 构造，
/// 避免 panel ↔ sub 错配（例如 Outline + FileTreeFocus）。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct PanelFocus {
    pub(crate) panel: PanelId,
    pub(crate) sub: PanelSubFocus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum PanelSubFocus {
    /// 没有 sub-focus —— panel 自身的容器焦点。
    Bare,
    FileTree(FileTreeFocus),
    /// 版本管理面板焦点。
    VersionControl(VersionControlFocus),
}

impl PanelFocus {
    pub(crate) fn bare(panel: PanelId) -> Self {
        Self {
            panel,
            sub: PanelSubFocus::Bare,
        }
    }

    pub(crate) fn file_tree(sub: FileTreeFocus) -> Self {
        Self {
            panel: PanelId::FileTree,
            sub: PanelSubFocus::FileTree(sub),
        }
    }

    pub(crate) fn version_control() -> Self {
        Self {
            panel: PanelId::VersionControl,
            sub: PanelSubFocus::VersionControl(VersionControlFocus::Navigate),
        }
    }

    pub(crate) fn version_control_commit() -> Self {
        Self {
            panel: PanelId::VersionControl,
            sub: PanelSubFocus::VersionControl(VersionControlFocus::CommitMessage),
        }
    }

    pub(crate) fn as_file_tree(self) -> Option<FileTreeFocus> {
        match self.sub {
            PanelSubFocus::FileTree(focus) => Some(focus),
            _ => None,
        }
    }

    pub(crate) fn as_version_control(self) -> Option<VersionControlFocus> {
        match self.sub {
            PanelSubFocus::VersionControl(focus) => Some(focus),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum FileTreeFocus {
    Navigate,
    NewEntryName,
    RenameEntry,
    ConfirmDelete,
}

/// 版本控制面板内的子焦点。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum VersionControlFocus {
    /// 树形导航。
    Navigate,
    /// 提交信息编辑。
    CommitMessage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum SearchField {
    Query,
    Replacement,
}

/// Surface 焦点：哪一个 surface + 该 surface 内的 sub-focus。
///
/// 与 [`PanelFocus`] 同理，构造只能走工厂方法。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct SurfaceFocus {
    pub(crate) surface: SurfaceId,
    pub(crate) sub: SurfaceSubFocus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum SurfaceSubFocus {
    /// 没有 sub-focus —— surface 自身的容器焦点。
    Bare,
}

impl SurfaceFocus {
    pub(crate) fn bare(surface: SurfaceId) -> Self {
        Self {
            surface,
            sub: SurfaceSubFocus::Bare,
        }
    }
}

impl AppFocus {
    pub(crate) fn editor() -> Self {
        Self::Editor(EditorFocus::Main)
    }

    pub(crate) fn file_tree(focus: FileTreeFocus) -> Self {
        Self::Panel(PanelFocus::file_tree(focus))
    }

    pub(crate) fn search(field: SearchField) -> Self {
        Self::SearchBar(field)
    }

    pub(crate) fn go_to_line() -> Self {
        Self::GoToLine
    }

    /// 当前焦点是否落在搜索栏的某个输入框；不是则 `None`。
    /// 各 `TextTargetOwner` / handler 从 `AppFocus` 抠 SearchField 都走这里。
    pub(crate) fn as_search(self) -> Option<SearchField> {
        match self {
            Self::SearchBar(field) => Some(field),
            _ => None,
        }
    }

    /// 无 sub-focus 的 panel 焦点（版本控制 / 大纲 / 终端 / 调试 / 键盘快捷键等）。
    pub(crate) fn panel(panel: PanelId) -> Self {
        Self::Panel(PanelFocus::bare(panel))
    }

    pub(crate) fn project_picker() -> Self {
        Self::Surface(SurfaceFocus::bare(SurfaceId::ProjectPicker))
    }

    pub(crate) fn settings() -> Self {
        Self::Surface(SurfaceFocus::bare(SurfaceId::Settings))
    }

    pub(crate) fn language_servers() -> Self {
        Self::Surface(SurfaceFocus::bare(SurfaceId::LanguageServers))
    }

    pub(crate) fn branch_picker() -> Self {
        Self::Surface(SurfaceFocus::bare(SurfaceId::BranchPicker))
    }

    /// 是否投影到同一个 GPUI 焦点宿主。
    /// panel/surface 相同即视为同投影，sub-focus 差异（如文件树内部模式）由 App 自行细化。
    /// 搜索栏的 query / replacement 是两个独立 handle，按 field 严格分。
    pub(crate) fn same_projection(self, other: Self) -> bool {
        match (self, other) {
            (Self::None, Self::None) => true,
            (Self::Editor(_), Self::Editor(_)) => true,
            (Self::Panel(a), Self::Panel(b)) => a.panel == b.panel,
            (Self::Surface(a), Self::Surface(b)) => a.surface == b.surface,
            (Self::SearchBar(a), Self::SearchBar(b)) => a == b,
            (Self::GoToLine, Self::GoToLine) => true,
            (left, right) => left == right,
        }
    }
}

/// App 端唯一焦点存储。`previous` 给 surface / modal 关闭时恢复焦点用。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FocusStore {
    current: AppFocus,
    previous: Option<AppFocus>,
}

impl FocusStore {
    pub(crate) fn new(current: AppFocus) -> Self {
        Self {
            current,
            previous: None,
        }
    }

    pub(crate) fn current(&self) -> AppFocus {
        self.current
    }

    pub(crate) fn request(&mut self, next: AppFocus) {
        if self.current == next {
            return;
        }
        self.previous = Some(self.current);
        self.current = next;
    }

    pub(crate) fn restore_previous(&mut self) -> AppFocus {
        let next = self.previous.take().unwrap_or_default();
        self.current = next;
        next
    }
}
