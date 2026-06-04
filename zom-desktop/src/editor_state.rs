//! 编辑区标签栏的渲染快照与构建。
//!
//! 这里只描述「工作台标签栏要显示哪些 tab」；正文 / 光标 / IME 属于
//! [`crate::text_target::EditorRouter`] 与 shell 的编辑器视图职责。

use zom_view::{ViewId, ViewSet};
use zom_workspace::{Workspace, WorkspaceBuffer};

#[derive(Clone, Debug, Default)]
pub(crate) struct EditorState {
    pub(crate) tabs: Vec<EditorTab>,
}

#[derive(Clone, Debug)]
pub(crate) struct EditorTab {
    pub(crate) id: ViewId,
    pub(crate) title: String,
    pub(crate) language: String,
    pub(crate) dirty: bool,
    pub(crate) is_active: bool,
}

pub(crate) fn build_editor_state(workspace: &Workspace, views: &ViewSet) -> EditorState {
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
