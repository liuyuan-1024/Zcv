//! 编辑区标签栏的渲染快照与构建。
//!
//! 这里只描述「工作台标签栏要显示哪些 tab」；
//! 正文 / 光标 / IME 属于 [`crate::text_target::EditorRouter`] 与 shell 的编辑器视图职责。
//!
//! 一条 view = 一条 tab：直接迭代 [`ViewSet`]，按 view 的 kind 决定 tab 的显示形式（编辑视图正常显示文件名 + dirty 标记；预览视图加 " · 预览" 后缀且不显示 dirty）。

use std::path::Path;

use zom_engine::ByteOffset;
use zom_view::{View, ViewId, ViewKind, ViewSet};
use zom_workspace::{Workspace, WorkspaceBuffer};

#[derive(Clone, Debug, Default)]
pub(crate) struct EditorState {
    pub(crate) tabs: Vec<EditorTab>,
}

#[derive(Clone, Debug)]
pub(crate) struct EditorTab {
    /// tab 的稳定身份：背后那条 view 的 id。
    pub(crate) view_id: ViewId,
    pub(crate) kind: ViewKind,
    pub(crate) title: String,
    pub(crate) language: String,
    pub(crate) dirty: bool,
    pub(crate) is_active: bool,
    /// 项目根的相对显示路径（统一正斜杠）。
    /// 无项目根或文件不在项目内时为 None。
    pub(crate) relative_path: Option<String>,
    /// 预览 tab 的整段源码文本——只在 `kind == Preview` 时填充，供 markdown 渲染器消费。
    /// 编辑 tab 永远为 None：编辑视图的正文由独立的编辑器子系统从 buffer 实时切片，不走这里。
    pub(crate) preview_text: Option<String>,
}

impl EditorTab {
    /// 给 GPUI `.id((..,key))` 用的稳定 key——直接用 ViewId 的数值即可，全局唯一。
    pub(crate) fn element_key(&self) -> u64 {
        self.view_id.as_u64()
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
) -> EditorState {
    let tabs = views
        .views()
        .map(|(view_id, view)| build_tab(workspace, project_root, view_id, view, active_view))
        .collect();
    EditorState { tabs }
}

fn build_tab(
    workspace: &Workspace,
    project_root: Option<&Path>,
    view_id: ViewId,
    view: &View,
    active_view: Option<ViewId>,
) -> EditorTab {
    let buffer_id = view.buffer();
    let buffer = workspace.buffer(buffer_id);
    let base_title = buffer
        .map(buffer_title)
        .unwrap_or_else(|| "未命名".to_string());
    let relative_path = buffer.and_then(|b| relative_display(b.path(), project_root));
    let kind = view.kind();
    let display_title = match kind {
        ViewKind::Edit => base_title.clone(),
        ViewKind::Preview => format!("{base_title} · 预览"),
    };
    let preview_text = match kind {
        ViewKind::Preview => buffer.and_then(read_full_text),
        ViewKind::Edit => None,
    };
    EditorTab {
        view_id,
        kind,
        language: language_label(&base_title),
        title: display_title,
        // 预览 tab 不显示脏标记——buffer 是否脏由对应的编辑 tab 反映。
        dirty: matches!(kind, ViewKind::Edit)
            && buffer.map(WorkspaceBuffer::is_dirty).unwrap_or(false),
        is_active: active_view == Some(view_id),
        relative_path,
        preview_text,
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

fn language_label(title: &str) -> String {
    match std::path::Path::new(title)
        .extension()
        .and_then(|ext| ext.to_str())
    {
        Some("rs") => "Rust".to_string(),
        Some("toml") | Some("lock") => "TOML".to_string(),
        Some("md") | Some("markdown") => "Markdown".to_string(),
        Some("json") => "JSON".to_string(),
        Some("js") | Some("mjs") | Some("cjs") => "JavaScript".to_string(),
        Some("ts") => "TypeScript".to_string(),
        Some("jsx") => "JSX".to_string(),
        Some("tsx") => "TSX".to_string(),
        Some("html") | Some("htm") => "HTML".to_string(),
        Some("css") => "CSS".to_string(),
        Some("scss") | Some("sass") => "Sass".to_string(),
        Some("yaml") | Some("yml") => "YAML".to_string(),
        Some("xml") => "XML".to_string(),
        Some("py") => "Python".to_string(),
        Some("go") => "Go".to_string(),
        Some("c") | Some("h") => "C".to_string(),
        Some("cpp") | Some("cc") | Some("cxx") | Some("hpp") => "C++".to_string(),
        Some("java") => "Java".to_string(),
        Some("kt") | Some("kts") => "Kotlin".to_string(),
        Some("swift") => "Swift".to_string(),
        Some("rb") => "Ruby".to_string(),
        Some("php") => "PHP".to_string(),
        Some("sh") | Some("bash") | Some("zsh") => "Shell".to_string(),
        Some("sql") => "SQL".to_string(),
        Some("ini") | Some("conf") | Some("cfg") => "INI".to_string(),
        Some("txt") | Some("text") => "Text".to_string(),
        Some("csv") => "CSV".to_string(),
        Some("svg") => "SVG".to_string(),
        Some(other) => other.to_uppercase(),
        None => "Unknown".to_string(),
    }
}
