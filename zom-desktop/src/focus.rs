//! 应用层焦点模型 —— 唯一语义真相源。
//!
//! 业务只认 [`AppFocus`]：命令分发、快捷键 context、IME 路由、状态栏显示全部从这个值派生，
//! 不再各自维护一套 role / target_id / active 优先级。
//!
//! GPUI 的 [`FocusHandle`](gpui::FocusHandle) 是它在窗口系统里的投影，对应桥接层在 [`crate::shell::view::focus`]；
//! 本模块不依赖 GPUI 类型，全部是纯数据。

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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum EditorFocus {
    Main,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum PanelFocus {
    FileTree(FileTreeFocus),
    Search(SearchField),
    VersionControl,
    Outline,
    Terminal,
    Debug,
    KeyboardShortcuts,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum FileTreeFocus {
    Navigate,
    NewEntryName,
    RenameEntry,
    ConfirmDelete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum SearchField {
    Query,
    Replacement,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum SurfaceFocus {
    ProjectPicker(ProjectPickerFocus),
    LanguageServers,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum ProjectPickerFocus {
    Query,
    RecentList,
}

impl AppFocus {
    pub(crate) fn editor() -> Self {
        Self::Editor(EditorFocus::Main)
    }

    pub(crate) fn file_tree(focus: FileTreeFocus) -> Self {
        Self::Panel(PanelFocus::FileTree(focus))
    }

    pub(crate) fn search(field: SearchField) -> Self {
        Self::Panel(PanelFocus::Search(field))
    }

    pub(crate) fn project_picker(focus: ProjectPickerFocus) -> Self {
        Self::Surface(SurfaceFocus::ProjectPicker(focus))
    }

    /// 是否投影到同一个 GPUI 焦点宿主。编辑器的 view id、文件树内部模式
    /// 这些 leaf 差异不影响系统焦点句柄。
    pub(crate) fn same_projection(self, other: Self) -> bool {
        match (self, other) {
            (Self::None, Self::None) => true,
            (Self::Editor(_), Self::Editor(_)) => true,
            (Self::Panel(PanelFocus::FileTree(_)), Self::Panel(PanelFocus::FileTree(_))) => true,
            (
                Self::Surface(SurfaceFocus::ProjectPicker(_)),
                Self::Surface(SurfaceFocus::ProjectPicker(_)),
            ) => true,
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
