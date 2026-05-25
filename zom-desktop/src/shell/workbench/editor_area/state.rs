//! 编辑区标签栏的渲染快照与构建。
//!
//! 这里只描述「工作台标签栏要显示哪些 tab」——正文 / 光标 / IME 属于
//! [`TextEditorSlot`](crate::shell::editor::TextEditorSlot) 与
//! [`EditorRouter`](crate::shell::editor::EditorRouter) 的职责，本模块不掺合。
//! 底栏需要"行:列"展示时另由 view 把
//! [`EditorSnapshot`](crate::shell::editor::EditorSnapshot) 作为渲染参数传入；
//! 闪烁经 [`CaretClock`](crate::shell::editor::CaretClock) 全局共享。

use zom_view::{ViewId, ViewSet};
use zom_workspace::{Workspace, WorkspaceBuffer};

/// 编辑区标签栏的渲染快照——仅描述工作台关心的标签列表。
#[derive(Clone, Debug, Default)]
pub(crate) struct EditorState {
    pub(crate) tabs: Vec<EditorTab>,
}

/// 编辑区一个标签的渲染摘要。
#[derive(Clone, Debug)]
pub(crate) struct EditorTab {
    /// 对应的 View；后续切换 / 关闭命令用它定位。
    pub(crate) id: ViewId,
    /// 标签显示名（文件名，无路径的 scratch 显示「未命名」）。
    pub(crate) title: String,
    /// 由文件名推断的语言显示名（底栏等 UI 直接展示，不再各自计算）。
    pub(crate) language: String,
    pub(crate) dirty: bool,
    pub(crate) is_active: bool,
}

/// 把 `ViewSet` 里的每个视图映射成一个标签摘要，顺序即打开顺序。
pub(crate) fn build(workspace: &Workspace, views: &ViewSet) -> EditorState {
    let active = views.active();
    let tabs = views
        .views()
        .map(|(id, view)| {
            let buffer = workspace.buffer(view.buffer());
            let title = buffer
                .map(buffer_title)
                .unwrap_or_else(|| "未命名".to_string());
            EditorTab {
                id,
                language: language_label(&title),
                title,
                dirty: buffer.map(WorkspaceBuffer::is_dirty).unwrap_or(false),
                is_active: Some(id) == active,
            }
        })
        .collect();
    EditorState { tabs }
}

/// 取 buffer 的标签显示名：有路径用文件名，无路径（scratch）显示「未命名」。
fn buffer_title(buffer: &WorkspaceBuffer) -> String {
    buffer
        .path()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "未命名".to_string())
}

/// 由文件名后缀推断语言显示名；未知后缀回退为大写后缀，无后缀为「Unknown」。
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
