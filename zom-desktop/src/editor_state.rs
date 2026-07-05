//! 编辑区标签栏的渲染快照与构建。
//!
//! 这里只描述「工作台标签栏要显示哪些 tab」；
//! 正文 / 光标 / IME 属于 [`crate::text_target::EditorRouter`] 与 shell 的编辑器视图职责。
//!
//! 一条 view = 一条 tab：直接迭代 [`ViewSet`]，按 view 的 kind 决定 tab 的显示形式（编辑视图正常显示文件名 + dirty 标记；预览视图加 " · 预览" 后缀且不显示 dirty）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::ScrollHandle;
use zom_engine::{BufferVersion, ByteOffset};
use zom_workspace::syntax::SyntaxEngine;
use zom_workspace::view::{View, ViewId, ViewKind, ViewSet};
use zom_workspace::{Workspace, WorkspaceBuffer};

#[derive(Clone, Debug, Default)]
pub(crate) struct EditorState {
    pub(crate) tabs: Vec<EditorTab>,
    /// 预览文本缓存：view_id → (buffer_version, cached_text)。
    /// 跨帧复用，避免每帧全量重读 buffer。
    pub(crate) preview_cache: BTreeMap<ViewId, (BufferVersion, String)>,
    /// 预览滚动句柄：view_id → ScrollHandle。
    /// 跨帧 / 跨 tab 切换复用，保持预览滚动位置。
    pub(crate) preview_scroll_handles: BTreeMap<ViewId, ScrollHandle>,
    /// 共享语法引擎：供 Markdown 预览代码块高亮使用。
    /// `None` 仅在无 workspace 时（单元测试 / 零文件启动）。
    pub(crate) syntax_engine: Option<Rc<SyntaxEngine>>,
}

// ── Tab 类型 ──

#[derive(Clone, Debug)]
pub(crate) enum EditorTab {
    Edit(EditTab),
    Preview(PreviewTab),
}

#[derive(Clone, Debug)]
pub(crate) struct EditTab {
    pub(crate) view_id: ViewId,
    pub(crate) title: String,
    pub(crate) is_active: bool,
    pub(crate) language: String,
    pub(crate) dirty: bool,
    pub(crate) relative_path: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PreviewTab {
    pub(crate) view_id: ViewId,
    pub(crate) title: String,
    pub(crate) is_active: bool,
    pub(crate) preview_text: String,
    pub(crate) buffer_path: Option<PathBuf>,
}

impl EditorTab {
    pub(crate) fn view_id(&self) -> ViewId {
        match self {
            EditorTab::Edit(t) => t.view_id,
            EditorTab::Preview(t) => t.view_id,
        }
    }

    pub(crate) fn title(&self) -> &str {
        match self {
            EditorTab::Edit(t) => &t.title,
            EditorTab::Preview(t) => &t.title,
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        match self {
            EditorTab::Edit(t) => t.is_active,
            EditorTab::Preview(t) => t.is_active,
        }
    }

    /// 给 GPUI `.id((..,key))` 用的稳定 key。
    pub(crate) fn element_key(&self) -> u64 {
        self.view_id().as_u64()
    }
}

/// 构造 tab bar 渲染快照。
///
/// 按 `views` 的 ViewId 升序遍历，每条 view 对应一条 tab；
/// `active_view` 决定哪条 tab 的 `is_active = true`。
pub(crate) fn build_editor_state(
    workspace: &Workspace,
    views: &ViewSet,
    active_view: Option<ViewId>,
    project_root: Option<&Path>,
    prev_cache: &BTreeMap<ViewId, (BufferVersion, String)>,
    prev_scroll_handles: &BTreeMap<ViewId, ScrollHandle>,
) -> EditorState {
    let mut preview_cache: BTreeMap<ViewId, (BufferVersion, String)> = BTreeMap::new();
    let mut preview_scroll_handles: BTreeMap<ViewId, ScrollHandle> = BTreeMap::new();
    for (view_id, handle) in prev_scroll_handles {
        preview_scroll_handles.insert(*view_id, handle.clone());
    }
    let tabs: Vec<EditorTab> = views
        .views()
        .map(|(view_id, view)| {
            build_tab(
                workspace,
                project_root,
                view_id,
                view,
                active_view,
                prev_cache,
                &mut preview_cache,
            )
        })
        .collect();
    // 为尚无句柄的预览 tab 创建新句柄。
    let live_ids: std::collections::BTreeSet<ViewId> = tabs.iter().map(|t| t.view_id()).collect();
    preview_scroll_handles.retain(|id, _| live_ids.contains(id));
    for tab in &tabs {
        if let EditorTab::Preview(p) = tab {
            preview_scroll_handles.entry(p.view_id).or_default();
        }
    }
    EditorState {
        tabs,
        preview_cache,
        preview_scroll_handles,
        syntax_engine: Some(workspace.engine().clone()),
    }
}

fn build_tab(
    workspace: &Workspace,
    project_root: Option<&Path>,
    view_id: ViewId,
    view: &View,
    active_view: Option<ViewId>,
    prev_cache: &BTreeMap<ViewId, (BufferVersion, String)>,
    out_cache: &mut BTreeMap<ViewId, (BufferVersion, String)>,
) -> EditorTab {
    let buffer = workspace.buffer(view.buffer());
    let base_title = buffer
        .map(buffer_title)
        .unwrap_or_else(|| "未命名".to_string());
    let is_active = active_view == Some(view_id);

    match view.kind() {
        ViewKind::Edit => {
            let relative_path = buffer.and_then(|b| relative_display(b.path(), project_root));
            EditorTab::Edit(EditTab {
                view_id,
                title: base_title,
                language: buffer
                    .and_then(|b| b.language())
                    .map(|lang| lang.display_name().to_string())
                    .unwrap_or_else(|| "Unknown".to_string()),
                dirty: buffer.map(WorkspaceBuffer::is_dirty).unwrap_or(false),
                is_active,
                relative_path,
            })
        }
        ViewKind::Preview => {
            let preview_text = buffer.and_then(|buf| {
                let version = buf.buffer().version();
                if let Some((cached_version, cached_text)) = prev_cache.get(&view_id)
                    && *cached_version == version
                {
                    out_cache.insert(view_id, (*cached_version, cached_text.clone()));
                    return Some(cached_text.clone());
                }
                let text = read_full_text(buf)?;
                out_cache.insert(view_id, (version, text.clone()));
                Some(text)
            });
            EditorTab::Preview(PreviewTab {
                view_id,
                title: format!("{base_title} · 预览"),
                is_active,
                preview_text: preview_text.unwrap_or_default(),
                buffer_path: buffer.and_then(|b| {
                    b.path()
                        .and_then(|p| p.parent().map(|parent| parent.to_path_buf()))
                }),
            })
        }
    }
}

/// 把整个 buffer 读成 owned `String`——只给预览 tab 用，调用频率随渲染节奏。
///
/// 预览路径目前每帧重渲染，故每帧拷贝一份 buffer 文本；
/// 大文档下 CPU 偏高时再做"按 BufferVersion 缓存"的版本。
fn read_full_text(buffer: &WorkspaceBuffer) -> Option<String> {
    let inner = buffer.buffer();
    inner
        .slice_byte_range(ByteOffset::ZERO, inner.len_bytes())
        .ok()
        .map(|slice| slice.into_text().into_owned())
}

/// 把 buffer 绝对路径折成项目相对、正斜杠显示串。
///
/// 项目根缺失、或文件不在项目根之下时返回 None——状态栏会回退到 tab 文件名。
fn relative_display(path: Option<&Path>, project_root: Option<&Path>) -> Option<String> {
    let path = path?;
    let root = project_root?;
    let rel = path.strip_prefix(root).ok()?;
    let mut out = String::new();
    for (i, part) in rel.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        out.push_str(&part.to_string_lossy());
    }
    if out.is_empty() { None } else { Some(out) }
}

fn buffer_title(buffer: &WorkspaceBuffer) -> String {
    buffer
        .path()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "未命名".to_string())
}
