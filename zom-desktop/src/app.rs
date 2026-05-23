//! app —— 组合根（手册 2 / 13）。
//!
//! P2 最小编辑闭环已接入：组合根持有 `CommandRegistry`、`Keymap`、
//! `Workspace` 与 `ViewSet`，并把输入统一收敛到 command 管线。
//!
//! 依赖方向（手册 2.4）：`app` 可以 import `shell`；`shell` 不可反向 import `app`。
//! 本文件只做组合根职责；具体功能尽量回到各自 feature / editor / workbench。

use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zom_command::commands::{self, editor};
use zom_command::{
    CommandArgs, CommandContext, CommandError, CommandExecutor, CommandId, CommandQueue,
    CommandRegistry, EffectQueue, FileTreeKeyMode, HostEffect, Invocation, KeyChord, KeyContext,
    Keymap, KeymapResolution,
};
use zom_view::{ViewId, ViewSet};
use zom_workspace::{EntryKind, Workspace, WorkspaceBuffer};

use crate::shell::editor::{ImeQueryTarget, ImeTarget};
use crate::shell::features::file_tree::{FileTreeActivation, FileTreeModel, FileTreeState};

/// 主编辑区渲染快照：标签列表 + 当前活动 buffer 的正文。
#[derive(Clone, Debug, Default)]
pub(crate) struct EditorState {
    pub(crate) tabs: Vec<EditorTab>,
    pub(crate) text: String,
    pub(crate) cursor_byte: usize,
    /// 光标当前是否可见。闪烁是 shell 层职责，由根视图注入；`App` 恒留默认值。
    pub(crate) caret_visible: bool,
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

/// 顶栏项目选择器使用的最近项目摘要。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecentProject {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) identifier: String,
    pub(crate) repo: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct RecentProjectsFile {
    schema_version: u32,
    projects: Vec<RecentProjectRecord>,
}

#[derive(Debug, Deserialize, Serialize)]
struct RecentProjectRecord {
    name: String,
    path: PathBuf,
    identifier: String,
    repo: Option<String>,
}

/// 一次按键派发的结果。`consumed=false` 表示这次按键没有匹配任何 keymap
/// 绑定，应当透传给系统输入法；否则会阻塞 IME 的整个文本输入路径。
pub(crate) struct KeyDispatchOutcome {
    pub(crate) consumed: bool,
    pub(crate) effects: Vec<HostEffect>,
}

/// 一次按键来自哪个交互面。组合根据此 + 运行态算出 keymap 上下文栈，
/// 命令与快捷键的定义本身全在 zom-command。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KeySurface {
    /// 主编辑区。
    Editor,
    /// 普通面板。暂未接入专属键位时，只响应全局快捷键。
    Panel,
    /// 文件树面板（含新建条目输入态）。
    FileTree,
}

pub struct App {
    registry: CommandRegistry,
    keymap: Keymap,
    executor: CommandExecutor,
    queue: CommandQueue,
    workspace: Workspace,
    views: ViewSet,
    project_root: Option<PathBuf>,
    recent_projects: Vec<RecentProject>,
    recent_projects_path: Option<PathBuf>,
    file_tree: FileTreeModel,
}

impl App {
    /// 内存模式（测试版使用，避免污染真实目录）
    pub fn new() -> Self {
        Self::new_with_recent_projects_path(None)
    }

    /// 持久化模式（发行版使用）
    pub fn new_persistent() -> Self {
        Self::new_with_recent_projects_path(default_recent_projects_path())
    }

    pub(crate) fn new_with_recent_projects_path(path: Option<PathBuf>) -> Self {
        let mut registry = CommandRegistry::new();
        let mut keymap = Keymap::new();

        // 组合根只选择安装内建命令集；具体 feature catalog 的完整性由
        // zom-command 自己维护。宿主侧资源（窗口、Dock）走 HostEffect 反馈到 shell。
        commands::install_all(&mut registry, &mut keymap);

        let (workspace, views) = empty_workspace();
        let recent_projects = path
            .as_deref()
            .map(load_recent_projects)
            .unwrap_or_default();

        Self {
            registry,
            keymap,
            executor: CommandExecutor::new(),
            queue: CommandQueue::new(),
            workspace,
            views,
            project_root: None,
            recent_projects,
            recent_projects_path: path,
            file_tree: FileTreeModel::default(),
        }
    }

    pub(crate) fn open_local_project(&mut self, root: PathBuf) {
        self.open_project(root, None);
    }

    pub(crate) fn open_git_project(&mut self, root: PathBuf, repo: String) {
        self.open_project(root, Some(repo));
    }

    fn open_project(&mut self, root: PathBuf, repo: Option<String>) {
        self.file_tree.open_project(root.clone());
        self.remember_project(root.clone(), repo.clone());
        self.project_root = Some(root);
        let (workspace, views) = empty_workspace();
        self.workspace = workspace;
        self.views = views;
    }

    pub(crate) fn project_title(&self) -> String {
        self.project_root
            .as_deref()
            .and_then(project_name)
            .unwrap_or("打开项目")
            .to_string()
    }

    pub(crate) fn has_project(&self) -> bool {
        self.project_root.is_some()
    }

    pub(crate) fn recent_projects(&self) -> Vec<RecentProject> {
        self.recent_projects.clone()
    }

    pub(crate) fn remove_recent_project(&mut self, id: &str) {
        self.recent_projects.retain(|project| project.id != id);
        self.persist_recent_projects();
    }

    pub(crate) fn file_tree_state(&self) -> FileTreeState {
        self.file_tree.state(&self.workspace)
    }

    pub(crate) fn file_tree_ensure_selection_initialized(&mut self) {
        self.file_tree.ensure_selection_initialized();
    }

    pub(crate) fn file_tree_move_selection(&mut self, delta: isize) {
        self.file_tree.move_selection(delta);
    }

    pub(crate) fn file_tree_collapse_or_parent(&mut self) {
        self.file_tree.collapse_or_parent();
    }

    pub(crate) fn file_tree_expand_or_into(&mut self) {
        self.file_tree.expand_or_into();
    }

    pub(crate) fn file_tree_activate(&mut self) -> FileTreeActivation {
        self.file_tree
            .activate_selected(&mut self.workspace, &mut self.views)
    }

    pub(crate) fn file_tree_begin_new_entry(&mut self, kind: EntryKind) {
        self.file_tree.begin_new_entry(kind);
    }

    pub(crate) fn file_tree_cancel_new_entry(&mut self) {
        self.file_tree.cancel_new_entry();
    }

    /// 提交新建条目。新建文件会被立即打开，返回的 [`FileTreeActivation`] 让
    /// shell 据此把焦点切到编辑器。
    pub(crate) fn file_tree_commit_new_entry(&mut self) -> FileTreeActivation {
        self.file_tree
            .commit_new_entry(&mut self.workspace, &mut self.views)
    }

    pub(crate) fn file_tree_request_delete(&mut self) {
        self.file_tree.request_delete();
    }

    pub(crate) fn file_tree_confirm_delete(&mut self) {
        self.file_tree
            .confirm_delete(&mut self.workspace, &mut self.views);
    }

    pub(crate) fn file_tree_cancel_delete(&mut self) {
        self.file_tree.cancel_delete();
    }

    pub(crate) fn editor_state(&self) -> EditorState {
        let tabs = self.editor_tabs();

        let Some(view) = self.views.active_view() else {
            return EditorState {
                tabs,
                ..EditorState::default()
            };
        };
        let Some(buffer) = self.workspace.buffer(view.buffer()) else {
            return EditorState {
                tabs,
                ..EditorState::default()
            };
        };

        EditorState {
            tabs,
            text: buffer.buffer().text().into_owned(),
            cursor_byte: view.selection().primary().head().get(),
            // 闪烁由 shell 层注入，App 不参与。
            caret_visible: false,
        }
    }

    /// 把 `ViewSet` 里的每个视图映射成一个标签摘要，顺序即打开顺序。
    fn editor_tabs(&self) -> Vec<EditorTab> {
        let active = self.views.active();
        self.views
            .views()
            .map(|(id, view)| {
                let buffer = self.workspace.buffer(view.buffer());
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
            .collect()
    }

    /// 派发一次命令调用。
    ///
    /// 调用方应当来自 typed builder（如 `editor::insert_text("hi")`），从而避免
    /// 在调用点手写 `CommandId::new(...)` 或 `CommandArgs::new().with(...)`。
    pub(crate) fn dispatch(
        &mut self,
        invocation: Invocation,
    ) -> Result<Vec<HostEffect>, CommandError> {
        let (id, args) = invocation;
        self.dispatch_command_id(id, args)
    }

    /// 处理一次归一化按键。
    ///
    /// 宿主只传「按键来自哪个交互面」，由组合根按当前焦点 / 运行态算出
    /// `KeyContext` 栈交给 keymap 解析 —— 命令与快捷键的定义全在 zom-command，
    /// 宿主不持有任何 chord → 动作 的映射表。
    ///
    /// 文本输入不在这里 fallback：交给 GPUI 的 `EntityInputHandler` 路径，由
    /// 系统输入法或 NSTextInputClient 把文本喂给 `App::ime_*`。
    pub(crate) fn dispatch_key(
        &mut self,
        chord: String,
        surface: KeySurface,
    ) -> Result<KeyDispatchOutcome, CommandError> {
        // 组合态下宿主完全让位给系统输入法：不解析、不消费、不 stop_propagation。
        // 一旦拦下某个键（如 Esc → ime_cancel），系统 IME 会话就和我们脱节，
        // 它会再吞掉一个后续按键 —— 表现为「取消候选后要多按一次 Esc 才退出
        // 新建」。组合的更新 / 提交 / 取消都由 IME 回调（`ime_*`）驱动。
        if self.is_composing() {
            return Ok(KeyDispatchOutcome {
                consumed: false,
                effects: Vec::new(),
            });
        }
        let chord = KeyChord::new(chord)?;
        let contexts = self.key_contexts(surface);
        match self.keymap.resolve(&[chord], &contexts) {
            KeymapResolution::Matched { command, args } => {
                let effects = self.dispatch_command_id(command, args)?;
                Ok(KeyDispatchOutcome {
                    consumed: true,
                    effects,
                })
            }
            KeymapResolution::Pending => Ok(KeyDispatchOutcome {
                consumed: true,
                effects: Vec::new(),
            }),
            KeymapResolution::NoMatch => Ok(KeyDispatchOutcome {
                consumed: false,
                effects: Vec::new(),
            }),
        }
    }

    /// 把「当前焦点面 + 运行态」映射成 keymap 解析用的 `KeyContext` 优先级栈。
    ///
    /// 这是宿主该做的事 —— 告诉 zom-command「现在处于什么上下文」；至于哪个
    /// chord 对应哪条命令，仍由各 catalog 注册进 keymap 的绑定决定。
    ///
    /// `composing` 恒为 `false`：`dispatch_key` 在组合态直接让位给系统输入法，
    /// 根本不会走到这里。组合上下文（`KeyContext::text_edit` 的第二参）保留
    /// 在签名里，待将来真有「宿主侧处理组合键」的需求再启用。
    fn key_contexts(&self, surface: KeySurface) -> Vec<KeyContext> {
        match surface {
            KeySurface::Editor => {
                vec![KeyContext::text_edit(true, false), KeyContext::global()]
            }
            KeySurface::Panel => vec![KeyContext::global()],
            KeySurface::FileTree if self.file_tree.pending_delete_active() => vec![
                // 删除确认弹窗打开中：只解析确认 / 取消，导航键全部冻结。
                KeyContext::file_tree(FileTreeKeyMode::PendingDelete),
                KeyContext::global(),
            ],
            KeySurface::FileTree if self.file_tree.pending_active() => vec![
                // 新建条目输入态：单行编辑器先吃编辑键，未命中再落到文件树的
                // 确认 / 取消，最后才是全局快捷键。
                KeyContext::text_edit(false, false),
                KeyContext::file_tree(FileTreeKeyMode::PendingName),
                KeyContext::global(),
            ],
            KeySurface::FileTree => vec![
                KeyContext::file_tree(FileTreeKeyMode::Navigate),
                KeyContext::global(),
            ],
        }
    }

    /// 当前聚焦的编辑目标是否处于「有 preedit 的」输入法组合态。
    ///
    /// 空 preedit 不算 —— 系统输入法取消候选后会把 preedit 清空、但 composition
    /// 壳可能仍在。若空壳也算组合态，`dispatch_key` 会一直让位、keymap 再也
    /// 不接管，后续 Esc 永远到不了 `cancel_new_entry`。
    fn is_composing(&self) -> bool {
        self.focused_ime_query_target()
            .and_then(|target| target.preedit_text())
            .is_some_and(|preedit| !preedit.is_empty())
    }

    /// 查询某条命令的快捷键文案 —— 给 Glyph / 命令面板 / 菜单用。
    pub(crate) fn shortcut_for(&self, command_id: &str) -> Option<String> {
        let command = CommandId::new(command_id).ok()?;
        self.keymap.format_shortcut_for(&command)
    }

    /// 查询某条命令的显示标题 —— UI 不再为命令入口重复维护文案。
    pub(crate) fn command_title_for(&self, command_id: &str) -> Option<String> {
        let command = CommandId::new(command_id).ok()?;
        self.registry
            .command(&command)
            .map(|command| command.title.clone())
    }

    fn dispatch_command_id(
        &mut self,
        id: CommandId,
        args: CommandArgs,
    ) -> Result<Vec<HostEffect>, CommandError> {
        self.queue.dispatch(id, args);

        let mut effects = EffectQueue::new();
        let focused_field = self.file_tree.pending_edit_target();
        let mut context = CommandContext {
            workspace: &mut self.workspace,
            views: &mut self.views,
            focused_field,
            queue: &mut self.queue,
            effects: &mut effects,
        };
        let result = self.executor.run(&self.registry, &mut context);

        let host_effects = effects.drain();
        result?;
        Ok(host_effects)
    }

    /// 提交系统输入法文本。commit 走命令路径，保证进入 undo 历史。
    pub(crate) fn ime_replace_text(
        &mut self,
        replacement_range_utf16: Option<Range<usize>>,
        text: &str,
    ) -> Result<(), CommandError> {
        self.with_focused_ime_target(|mut target| {
            target.apply_replacement_range(replacement_range_utf16)
        })?;

        self.dispatch(editor::ime_commit(text))?;
        Ok(())
    }

    /// 更新输入法 preedit。update 走直接通道，避免每次按键都过命令队列。
    pub(crate) fn ime_replace_and_mark_text(
        &mut self,
        replacement_range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
    ) -> Result<(), CommandError> {
        self.with_focused_ime_target(|mut target| {
            target.replace_and_mark_text(
                replacement_range_utf16,
                new_text,
                new_selected_range_utf16,
            )
        })
    }

    pub(crate) fn ime_unmark(&mut self) -> Result<(), CommandError> {
        let preedit = self
            .focused_ime_query_target()
            .and_then(|target| target.preedit_text());
        let Some(preedit) = preedit else {
            return Ok(());
        };
        self.dispatch(editor::ime_commit(preedit))?;
        Ok(())
    }

    pub(crate) fn ime_marked_range_utf16(&self) -> Option<Range<usize>> {
        self.focused_ime_query_target()
            .and_then(|target| target.marked_range_utf16())
    }

    pub(crate) fn ime_selected_range_utf16(&self) -> Option<(Range<usize>, bool)> {
        self.focused_ime_query_target()
            .map(|target| target.selected_range_utf16())
    }

    pub(crate) fn ime_text_for_range_utf16(&self, range_utf16: Range<usize>) -> Option<String> {
        self.focused_ime_query_target()
            .and_then(|target| target.text_for_range_utf16(range_utf16))
    }

    fn with_focused_ime_target<R>(
        &mut self,
        f: impl FnOnce(ImeTarget<'_>) -> Result<R, CommandError>,
    ) -> Result<R, CommandError> {
        if let Some(pending_editor) = self.file_tree.pending_editor_mut() {
            return f(pending_editor.as_ime_target());
        }
        self.with_active_ime_target(f)
    }

    fn with_active_ime_target<R>(
        &mut self,
        f: impl FnOnce(ImeTarget<'_>) -> Result<R, CommandError>,
    ) -> Result<R, CommandError> {
        let buffer_id = self
            .views
            .active_view()
            .map(|view| view.buffer())
            .ok_or(CommandError::NoActiveView)?;
        let buffer = self
            .workspace
            .buffer_mut(buffer_id)
            .ok_or(CommandError::BufferNotFound(buffer_id))?
            .buffer_mut();
        let selection = self
            .views
            .active_view_mut()
            .ok_or(CommandError::NoActiveView)?
            .selection_mut();
        f(ImeTarget::new(buffer, selection))
    }

    fn focused_ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
        if let Some(pending_editor) = self.file_tree.pending_editor() {
            return Some(pending_editor.as_ime_query_target());
        }
        self.active_ime_query_target()
    }

    fn active_ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
        let view = self.views.active_view()?;
        let buffer = self.workspace.buffer(view.buffer())?.buffer();
        Some(ImeQueryTarget::new(buffer, view.selection()))
    }

    fn remember_project(&mut self, root: PathBuf, repo: Option<String>) {
        let id = project_id(&root);
        self.recent_projects.retain(|project| project.id != id);
        self.recent_projects.insert(
            0,
            RecentProject {
                id,
                name: project_name(&root).unwrap_or("未命名项目").to_string(),
                identifier: repo
                    .clone()
                    .unwrap_or_else(|| root.to_string_lossy().into_owned()),
                path: root,
                repo,
            },
        );
        self.persist_recent_projects();
    }

    fn persist_recent_projects(&self) {
        let Some(path) = &self.recent_projects_path else {
            return;
        };
        if let Err(error) = save_recent_projects(path, &self.recent_projects) {
            eprintln!("写入最近项目失败：{error}");
        }
    }
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

/// 空工作区：不预建任何 buffer/view。
///
/// 早期版本会默认开一个空白 scratch buffer，但它对用户没有意义、还会让编辑区
/// 显示一个不存在的"文件"，误导用户。现在编辑区在无活动视图时走
/// `EditorState::default()`，文件从文件树打开后才有内容。
fn empty_workspace() -> (Workspace, ViewSet) {
    (Workspace::new(), ViewSet::new())
}

fn project_name(path: &Path) -> Option<&str> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
}

fn project_id(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn default_recent_projects_path() -> Option<PathBuf> {
    Some(home_dir()?.join(".zom/recent_workspaces.toml"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

fn load_recent_projects(path: &Path) -> Vec<RecentProject> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            eprintln!("读取最近项目失败：{error}");
            return Vec::new();
        }
    };
    let file = match toml::from_str::<RecentProjectsFile>(&text) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("解析最近项目失败：{error}");
            return Vec::new();
        }
    };

    file.projects
        .into_iter()
        .filter(|record| !record.path.as_os_str().is_empty())
        .map(|record| {
            let id = project_id(&record.path);
            RecentProject {
                id,
                name: if record.name.is_empty() {
                    project_name(&record.path)
                        .unwrap_or("未命名项目")
                        .to_string()
                } else {
                    record.name
                },
                identifier: if record.identifier.is_empty() {
                    record
                        .repo
                        .clone()
                        .unwrap_or_else(|| record.path.to_string_lossy().into_owned())
                } else {
                    record.identifier
                },
                path: record.path,
                repo: record.repo,
            }
        })
        .collect()
}

fn save_recent_projects(path: &Path, projects: &[RecentProject]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建最近项目目录 {}：{error}", parent.display()))?;
    }

    let file = RecentProjectsFile {
        schema_version: 1,
        projects: projects
            .iter()
            .map(|project| RecentProjectRecord {
                name: project.name.clone(),
                path: project.path.clone(),
                identifier: project.identifier.clone(),
                repo: project.repo.clone(),
            })
            .collect(),
    };
    let text =
        toml::to_string_pretty(&file).map_err(|error| format!("无法序列化最近项目：{error}"))?;
    fs::write(path, text)
        .map_err(|error| format!("无法写入最近项目文件 {}：{error}", path.display()))
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
