//! VersionControl —— L3 panel：git 变更文件树。
//!
//! 只显示有 git 变更的文件（Modified、Added、Deleted、Untracked、Conflict），以目录树形式组织，支持折叠/展开。
//!
//! 数据来源 [`GitService`](crate::git_service::GitService)，树结构与 terminal_mask 计算参照 [`ProjectTree`](zom_workspace::ProjectTree)。

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{AnyElement, Context, Div, FocusHandle, IntoElement, Window, div, prelude::*};
use zom_command::{BubbleRequest, EditTarget, KeyContext, PanelKind, VersionControlKeyMode};

use crate::editor::TextEditorSlot;
use crate::editor::text::{
    EditorSnapshot, EditorSnapshotRequest, ImeQueryTarget, OwnedEditorTarget,
};
use crate::focus::{AppFocus, PanelFocus, VersionControlFocus};
use crate::git_service::{ColorKind, GitService, GitStatus, StatusCode};
use crate::host_intent::KeyRequest;
use crate::shell::CommandTitleLookup;
use crate::shell::normalized_chord;
use crate::shell::shared::scroll;
use crate::shell::workbench::docks::{placeholder, render_focus_host};
use crate::text_target::{TextTargetOwner, TextTargetQuery};

mod effects;
mod view;

pub(crate) use effects::try_apply_effect;

const COMMAND: &str = PanelKind::VersionControl.toggle_command_id();

// ── 内部条目（建树用） ──

/// 建树阶段的条目——存于 BTreeMap，按路径自动排序。
#[derive(Clone, Debug)]
struct VcEntry {
    /// 显示名（文件名或目录名）。
    name: String,
    /// 是否为目录。目录自身不在 GitService.statuses 中但由祖先推导得出。
    is_dir: bool,
    /// git 颜色：文件取自身 status，目录取子项最严重状态。
    git_color: Option<ColorKind>,
}

// ── 渲染行 ──

/// 渲染快照中的一行。
#[derive(Clone, Debug)]
pub(crate) struct VersionControlRow {
    pub path: PathBuf,
    pub name: String,
    /// 缩进深度（项目根直属子项 depth=1，孙子 depth=2……）。
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    pub git_color: Option<ColorKind>,
    /// 文件是否已在暂存区。目录行为 false。
    pub staged: bool,
}

/// 面板渲染快照。
#[derive(Clone, Debug, Default)]
pub(crate) struct VersionControlState {
    pub rows: Rc<Vec<VersionControlRow>>,
    pub selected: Option<PathBuf>,
    pub has_project: bool,
    pub is_git_repo: bool,
    pub is_empty: bool,
    /// 变更统计：(增行数, 删行数)。
    pub diff_stats: (u32, u32),
    /// 是否所有文件已暂存。
    pub all_staged: bool,
}

// ── Model ──

struct VersionControlModel {
    git_service: Rc<RefCell<GitService>>,
    /// 仓库根目录绝对路径（取自 GitService）。
    root_path: PathBuf,
    /// 所有条目（变更文件 + 祖先目录），按路径排序。
    entries: BTreeMap<PathBuf, VcEntry>,
    /// 父目录 → 排序后的子路径列表。仅包含目录条目。
    children: HashMap<PathBuf, Vec<PathBuf>>,
    /// 当前展开的目录集合。首次建树时所有目录默认展开。
    expanded: HashSet<PathBuf>,
    /// 缓存的代际号与行列表。
    cached_generation: u64,
    cached_rows: Rc<Vec<VersionControlRow>>,
    cached_diff_stats: (u32, u32),
    /// 键盘焦点行。
    selected: Option<PathBuf>,
    /// 提交信息编辑缓冲区。
    commit_message: OwnedEditorTarget,
    /// 待弹出的反馈气泡。
    pending_bubbles: Vec<BubbleRequest>,
    /// 异步回调产生的延迟气泡（如 on_next_frame 中的 git 操作错误）。
    deferred_bubbles: Rc<RefCell<Vec<BubbleRequest>>>,
    /// 变更树滚动句柄，跨渲染帧持久化以保持滚动位置。
    scroll_handle: scroll::ScrollHandle,
}

/// 目录优先 + 字母序（不区分大小写）排序，与文件树一致。
fn sort_by_dir_then_name(entries: &BTreeMap<PathBuf, VcEntry>, paths: &mut [PathBuf]) {
    paths.sort_by(|a, b| {
        let a_is_dir = entries.get(a).map(|e| e.is_dir).unwrap_or(false);
        let b_is_dir = entries.get(b).map(|e| e.is_dir).unwrap_or(false);
        match (a_is_dir, b_is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => {
                let a_name = entries.get(a).map(|e| e.name.as_str()).unwrap_or("");
                let b_name = entries.get(b).map(|e| e.name.as_str()).unwrap_or("");
                a_name.to_lowercase().cmp(&b_name.to_lowercase())
            }
        }
    });
}

/// 同 [`sort_by_dir_then_name`]，但接受 `&PathBuf` 引用切片（用于顶层条目排序）。
fn sort_by_dir_entries(entries: &BTreeMap<PathBuf, VcEntry>, paths: &mut [&PathBuf]) {
    paths.sort_by(|a, b| {
        let a_is_dir = entries.get(*a).map(|e| e.is_dir).unwrap_or(false);
        let b_is_dir = entries.get(*b).map(|e| e.is_dir).unwrap_or(false);
        match (a_is_dir, b_is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => {
                let a_name = entries.get(*a).map(|e| e.name.as_str()).unwrap_or("");
                let b_name = entries.get(*b).map(|e| e.name.as_str()).unwrap_or("");
                a_name.to_lowercase().cmp(&b_name.to_lowercase())
            }
        }
    });
}

impl VersionControlModel {
    fn new(git_service: Rc<RefCell<GitService>>) -> Self {
        let root_path = git_service.borrow().repo_root_path().to_path_buf();
        Self {
            git_service,
            root_path,
            entries: BTreeMap::new(),
            children: HashMap::new(),
            expanded: HashSet::new(),
            cached_generation: 0,
            cached_rows: Rc::new(Vec::new()),
            cached_diff_stats: (0, 0),
            selected: None,
            commit_message: OwnedEditorTarget::new(),
            pending_bubbles: Vec::new(),
            deferred_bubbles: Rc::new(RefCell::new(Vec::new())),
            scroll_handle: scroll::ScrollHandle::new(),
        }
    }

    /// 从 GitService 重建 entries + children 索引。
    ///
    /// 只在 git generation 变化时调用。
    fn build_index(&mut self) {
        let svc = self.git_service.borrow();

        self.entries.clear();
        self.children.clear();

        // 收集有变更的文件路径（排除 Ignored）。
        let mut file_paths: Vec<PathBuf> = Vec::new();
        for (rel_path, status) in svc.statuses() {
            let color = status.color_kind();
            if color.is_none() || color == Some(ColorKind::Ignored) {
                continue;
            }
            let name = rel_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| rel_path.to_string_lossy().into_owned());
            self.entries.insert(
                rel_path.clone(),
                VcEntry {
                    name,
                    is_dir: false,
                    git_color: color,
                },
            );
            file_paths.push(rel_path.clone());
        }

        // 为每个文件路径补上祖先目录条目。
        for file_path in &file_paths {
            let mut ancestor = file_path.parent();
            while let Some(p) = ancestor {
                if p.as_os_str().is_empty() {
                    break;
                }
                if !self.entries.contains_key(p) {
                    let name = p
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| p.to_string_lossy().into_owned());
                    let git_color = svc.dir_colors().get(p).copied();
                    self.entries.insert(
                        p.to_path_buf(),
                        VcEntry {
                            name,
                            is_dir: true,
                            git_color,
                        },
                    );
                }
                ancestor = p.parent();
            }
        }

        // 构建 children 索引：遍历 entries，把每个条目挂到父目录下。
        for (path, entry) in &self.entries {
            if !entry.is_dir {
                if let Some(parent) = path.parent() {
                    if !parent.as_os_str().is_empty() && self.entries.contains_key(parent) {
                        self.children
                            .entry(parent.to_path_buf())
                            .or_default()
                            .push(path.clone());
                    }
                }
            }
        }
        for (path, entry) in &self.entries {
            if entry.is_dir {
                if let Some(parent) = path.parent() {
                    if !parent.as_os_str().is_empty() && self.entries.contains_key(parent) {
                        self.children
                            .entry(parent.to_path_buf())
                            .or_default()
                            .push(path.clone());
                    }
                }
            }
        }

        // 每个目录的子项排序：目录优先 + 字母序（不区分大小写），与文件树一致。
        let entries = &self.entries;
        for kids in self.children.values_mut() {
            sort_by_dir_then_name(entries, kids);
        }

        // 对新目录默认展开（保留已有展开状态中的条目，清除不再存在的）。
        let new_expanded: HashSet<PathBuf> = self
            .entries
            .keys()
            .filter(|p| {
                self.entries.get(*p).map(|e| e.is_dir).unwrap_or(false)
                    && self.expanded.contains(*p)
            })
            .cloned()
            .collect();
        self.expanded = new_expanded;
        // 对新出现的目录默认展开。根目录仅在首次时默认展开。
        if !self.expanded.contains(&self.root_path.clone()) {
            self.expanded.insert(self.root_path.clone());
        }
        for (path, entry) in &self.entries {
            if entry.is_dir && !self.expanded.contains(path) {
                self.expanded.insert(path.clone());
            }
        }
    }

    /// DFS 产出可见行。
    ///
    /// 第一行是仓库根目录（depth=0），其下为变更文件树。
    /// 只递归进入 expanded 集合中的目录。
    fn visible_rows(&self) -> Vec<VersionControlRow> {
        let mut rows = Vec::new();

        // 仓库根目录行（depth=0），与文件树一致。
        let svc = self.git_service.borrow();
        let root_name = svc.root_name();
        // 根目录颜色 = 所有变更中最严重的状态。
        let root_color = self.entries.values().filter_map(|e| e.git_color).max();
        drop(svc);
        let root_expanded = self.expanded.contains(&self.root_path.clone());
        rows.push(VersionControlRow {
            path: self.root_path.clone(),
            name: root_name,
            depth: 0,
            is_dir: true,
            expanded: root_expanded,
            git_color: root_color,
            staged: false,
        });

        // 收集顶层条目：parent 不在 entries 里的就是仓库根的直属子项。
        let mut root_entries: Vec<&PathBuf> = self
            .entries
            .keys()
            .filter(|path| {
                let parent_in_entries = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .map(|p| self.entries.contains_key(p))
                    .unwrap_or(false);
                !parent_in_entries
            })
            .collect();
        // 顶层排序：目录优先 + 字母序
        sort_by_dir_entries(&self.entries, &mut root_entries);

        for child in root_entries.iter() {
            self.collect_visible(child, 1, String::new(), None, &mut rows);
        }
        rows
    }

    fn collect_visible(
        &self,
        path: &Path,
        depth: usize,
        chain_prefix: String,
        chain_start: Option<PathBuf>,
        rows: &mut Vec<VersionControlRow>,
    ) {
        let Some(entry) = self.entries.get(path) else {
            return;
        };
        let is_dir = entry.is_dir;
        let explicitly_expanded = is_dir && self.expanded.contains(path);
        let in_chain = !chain_prefix.is_empty();
        let expanded = explicitly_expanded || in_chain;

        // 单子目录链省略：展开 + 只有一个子目录，或在链中继续向下。
        // VC 的 collect_visible 以 entry 自身为处理单元（不像文件树以"目录的子项"为单元），
        // 所以 chain_prefix 只含已省略的上层目录名，不含当前 entry 的路径。
        if self.single_dir_child(path) && (explicitly_expanded || in_chain) {
            let start = chain_start.clone().unwrap_or_else(|| path.to_path_buf());
            // 将当前 entry 名加入前缀（只含已省略的上层名）
            let mid = if chain_prefix.is_empty() {
                entry.name.clone()
            } else {
                format!("{}/{}", chain_prefix, entry.name)
            };
            let child = self.children.get(path).and_then(|k| k.first());
            if let Some(child) = child {
                if self.single_dir_child(child) {
                    // 子目录仍需省略 → 前缀带上当前名，继续向下
                    self.collect_visible(child, depth, mid, Some(start), rows);
                } else {
                    // 链末端：子目录名也拼进前缀，push 后正常递归
                    let child_entry = &self.entries[child];
                    let display_name = if mid.is_empty() {
                        child_entry.name.clone()
                    } else {
                        format!("{}/{}", mid, child_entry.name)
                    };
                    rows.push(VersionControlRow {
                        path: start,
                        name: display_name,
                        depth,
                        is_dir: child_entry.is_dir,
                        expanded: true,
                        git_color: child_entry.git_color,
                        staged: false,
                    });
                    // 链末端已作为链行 push，直接递归子项。
                    if let Some(kids) = self.children.get(child) {
                        for grandchild in kids.iter() {
                            self.collect_visible(grandchild, depth + 1, String::new(), None, rows);
                        }
                    }
                }
            }
            return;
        }

        // 不省略：push 本行
        let base_name = if chain_prefix.is_empty() {
            entry.name.clone()
        } else {
            format!("{}/{}", chain_prefix, entry.name)
        };
        let display_name = if is_dir && !expanded {
            self.folded_name(path, &base_name)
        } else {
            base_name
        };

        let staged = if is_dir {
            false
        } else {
            self.git_service
                .borrow()
                .statuses()
                .get(path)
                .map(|s| matches!(s, GitStatus::Tracked { index, .. } if *index != StatusCode::Unmodified))
                .unwrap_or(false)
        };

        rows.push(VersionControlRow {
            path: path.to_path_buf(),
            name: display_name,
            depth,
            is_dir,
            expanded,
            git_color: entry.git_color,
            staged,
        });

        if expanded {
            if let Some(kids) = self.children.get(path) {
                for child in kids.iter() {
                    self.collect_visible(child, depth + 1, String::new(), None, rows);
                }
            }
        }
    }

    /// 目录是否只有一个子目录（可被折叠省略）。
    fn single_dir_child(&self, path: &Path) -> bool {
        match self.children.get(path) {
            Some(kids) if kids.len() == 1 => self
                .entries
                .get(&kids[0])
                .map(|e| e.is_dir)
                .unwrap_or(false),
            _ => false,
        }
    }

    /// 沿已加载的单子目录链拼接折叠名。
    fn folded_name(&self, start: &Path, base: &str) -> String {
        let mut name = base.to_string();
        let mut current = start.to_path_buf();
        loop {
            let kids = match self.children.get(&current) {
                Some(k) if k.len() == 1 => k,
                _ => break,
            };
            let child_entry = match self.entries.get(&kids[0]) {
                Some(e) if e.is_dir => e,
                _ => break,
            };
            name = format!("{}/{}", name, child_entry.name);
            current = kids[0].clone();
        }
        name
    }

    /// 切换目录的折叠/展开状态。空路径表示根目录。
    fn toggle(&mut self, path: &Path) {
        let is_dir = path.as_os_str().is_empty()
            || self.entries.get(path).map(|e| e.is_dir).unwrap_or(false);
        if !is_dir {
            return;
        }
        if self.expanded.contains(path) {
            self.expanded.remove(path);
        } else {
            self.expanded.insert(path.to_path_buf());
        }
        self.cached_rows = Rc::new(self.visible_rows());
    }

    /// 获取当前状态快照。
    fn state(&mut self, has_project: bool) -> VersionControlState {
        if !has_project {
            return VersionControlState {
                selected: None,
                has_project: false,
                ..Default::default()
            };
        }

        let svc = self.git_service.borrow();
        let is_git_repo = svc.is_git_repo();
        let generation = svc.generation();
        let is_empty = svc
            .statuses()
            .iter()
            .all(|(_, s)| s.color_kind().is_none() || s.color_kind() == Some(ColorKind::Ignored));
        drop(svc);

        if !is_git_repo {
            self.cached_rows = Rc::new(Vec::new());
            return VersionControlState {
                rows: Rc::new(Vec::new()),
                selected: None,
                has_project: true,
                is_git_repo: false,
                is_empty: true,
                diff_stats: (0, 0),
                all_staged: false,
            };
        }

        // generation 变化 → 重建索引 + visible_rows
        if generation != self.cached_generation {
            self.build_index();
            self.cached_rows = Rc::new(self.visible_rows());
            self.cached_diff_stats = self.git_service.borrow().diff_stats();
            self.cached_generation = generation;
        }

        // 首次加载时自动选中根行
        if self.selected.is_none() {
            self.selected = self.cached_rows.first().map(|r| r.path.clone());
        }

        let all_staged = self
            .cached_rows
            .iter()
            .filter(|r| !r.is_dir)
            .all(|r| r.staged);

        VersionControlState {
            rows: self.cached_rows.clone(),
            selected: self.selected.clone(),
            has_project: true,
            is_git_repo: true,
            is_empty,
            diff_stats: self.cached_diff_stats,
            all_staged,
        }
    }

    fn toggle_selected(&mut self) {
        let path = self.selected.clone();
        if let Some(p) = path {
            self.toggle(&p);
        }
    }

    /// 激活选中行：目录 → 折叠展开，文件 → 返回绝对路径供上层打开。
    fn activate_selected(&mut self) -> Option<PathBuf> {
        let path = self.selected.clone()?;
        let is_dir =
            self.entries.get(&path).map(|e| e.is_dir).unwrap_or(false) || path == self.root_path;
        if is_dir {
            self.toggle(&path);
            None
        } else {
            // 路径是相对于 repo_root 的，拼回绝对路径。
            let abs = self.git_service.borrow().repo_root_path().join(&path);
            Some(abs)
        }
    }

    /// 切换文件的暂存状态：未暂存 → `git add`，已暂存 → `git reset HEAD`。
    /// 目录不受理（无意义）。
    ///
    /// 先乐观翻转内存状态并立即刷新 UI（复选框瞬间响应），
    /// 再把真实 git 操作推迟到下一帧执行，避免阻塞渲染。
    fn toggle_stage(&mut self, path: &Path, window: &mut Window) {
        let is_dir = self.entries.get(path).map(|e| e.is_dir).unwrap_or(false);
        if is_dir {
            return;
        }

        let svc = self.git_service.borrow();
        let staged = svc
            .statuses()
            .get(path)
            .map(|s| matches!(s, GitStatus::Tracked { index, .. } if *index != StatusCode::Unmodified))
            .unwrap_or(false);
        drop(svc);

        // ① 乐观翻转内存状态 → 下一帧渲染时复选框已更新。
        self.git_service
            .borrow_mut()
            .flip_staged_in_memory(path, !staged);
        self.cached_generation = 0;
        window.refresh();

        // ② 推迟真实 git 操作到下一帧，不阻塞当前帧的渲染。
        let git = self.git_service.clone();
        let path = path.to_path_buf();
        let target_staged = !staged;
        let deferred = self.deferred_bubbles.clone();
        window.on_next_frame(move |_window, _cx| {
            let result = if target_staged {
                git.borrow().stage_file(&path)
            } else {
                git.borrow().unstage_file(&path)
            };
            if let Err(e) = result {
                deferred.borrow_mut().push(
                    BubbleRequest::error(format!(
                        "git stage/unstage 失败（{}）：{e}",
                        path.display()
                    ))
                    .dedupe("vc.stage"),
                );
            }
            // 用真实 git 状态校准内存数据。
            let _ = git.borrow_mut().refresh_single(&path);
        });
    }

    fn move_selection(&mut self, delta: isize) {
        let rows = &self.cached_rows;
        if rows.is_empty() {
            return;
        }
        let current_idx = self
            .selected
            .as_ref()
            .and_then(|p| rows.iter().position(|r| &r.path == p))
            .unwrap_or(0);
        let new_idx = ((current_idx as isize + delta).max(0) as usize).min(rows.len() - 1);
        self.selected = Some(rows[new_idx].path.clone());
    }

    /// 文件树同款逻辑：展开目录→折叠；否则→跳到父目录。
    fn collapse_or_parent(&mut self) {
        let Some(selected) = self.selected.clone() else {
            return;
        };
        // 根目录 → 折叠。
        if selected.as_os_str().is_empty() {
            if self.expanded.contains(&selected) {
                self.expanded.remove(&selected);
                self.cached_rows = Rc::new(self.visible_rows());
            }
            return;
        }
        // 展开的目录 → 折叠。
        if self
            .entries
            .get(&selected)
            .map(|e| e.is_dir)
            .unwrap_or(false)
            && self.expanded.contains(&selected)
        {
            self.expanded.remove(&selected);
            self.cached_rows = Rc::new(self.visible_rows());
            return;
        }
        // 文件或折叠目录 → 跳到父目录。
        if let Some(parent) = selected.parent() {
            if !parent.as_os_str().is_empty() && self.entries.contains_key(parent) {
                self.selected = Some(parent.to_path_buf());
            }
        }
    }

    /// 文件树同款逻辑：折叠目录→展开；展开目录→进入首个子项。
    fn expand_or_into(&mut self) {
        let Some(selected) = self.selected.clone() else {
            return;
        };
        let Some(entry) = self.entries.get(&selected) else {
            return;
        };
        if !entry.is_dir {
            return;
        }
        if self.expanded.contains(&selected) {
            // 已展开 → 跳入第一个子项。
            if let Some(kids) = self.children.get(&selected) {
                if let Some(first) = kids.first() {
                    self.selected = Some(first.clone());
                }
            }
        } else {
            // 折叠 → 展开。
            self.expanded.insert(selected);
            self.cached_rows = Rc::new(self.visible_rows());
        }
    }

    /// 读取提交信息文本。
    fn commit_text(&self) -> String {
        self.commit_message.text()
    }

    /// 清空提交信息。
    fn clear_commit_message(&mut self) {
        self.commit_message = OwnedEditorTarget::new();
    }

    /// 执行 git commit 的核心逻辑：校验非空 → 提交 → 清空编辑区 → 弹气泡 → 刷新 git。
    /// 调用方负责焦点切换和 window.refresh()。
    fn try_commit(&mut self) {
        let msg = self.commit_text();
        if msg.trim().is_empty() {
            self.pending_bubbles
                .push(BubbleRequest::error("提交信息不能为空").dedupe("vc.commit.empty"));
            return;
        }
        let git = self.git_service.clone();
        // 把 commit 结果提取到局部变量——match 表达式中 git.borrow() 的
        // 临时 Ref 会存活到整个 match 结束，与 Ok 分支内 borrow_mut() 冲突。
        let result = git.borrow().commit(&msg);
        match result {
            Ok(()) => {
                self.clear_commit_message();
                self.pending_bubbles
                    .push(BubbleRequest::info("提交成功").dedupe("vc.commit.success"));
                let _ = git.borrow_mut().refresh();
            }
            Err(e) => {
                self.pending_bubbles
                    .push(BubbleRequest::error(format!("提交失败：{e}")).dedupe("vc.commit.error"));
            }
        }
    }

    /// 消费待弹出的反馈气泡（包含同步和异步两个来源）。
    fn take_pending_bubbles(&mut self) -> Vec<BubbleRequest> {
        let mut bubbles = std::mem::take(&mut self.pending_bubbles);
        bubbles.append(&mut self.deferred_bubbles.borrow_mut());
        bubbles
    }
}

// ── TextTarget 协议 ──

fn vc_commit_focus(focus: AppFocus) -> bool {
    matches!(
        focus,
        AppFocus::Panel(p) if matches!(p.as_version_control(), Some(VersionControlFocus::CommitMessage))
    )
}

impl TextTargetQuery for VersionControlModel {
    fn accepts_focus(&self, focus: AppFocus) -> bool {
        vc_commit_focus(focus)
    }

    fn snapshot(&self, _focus: AppFocus) -> EditorSnapshot {
        self.commit_message
            .snapshot(EditorSnapshotRequest::viewport(0, 5))
    }

    fn key_contexts(&self) -> Vec<KeyContext> {
        vec![
            KeyContext::version_control(VersionControlKeyMode::CommitMessage),
            KeyContext::text_edit(true, false),
            KeyContext::global(),
        ]
    }

    fn accepts_newline(&self) -> bool {
        true
    }

    fn ime_query_target(&self, _focus: AppFocus) -> Option<ImeQueryTarget<'_>> {
        Some(self.commit_message.as_ime_query_target())
    }
}

impl TextTargetOwner for VersionControlModel {
    fn edit_target(&mut self, _focus: AppFocus) -> Option<EditTarget<'_>> {
        Some(self.commit_message.as_edit_target())
    }
}

// ── Runtime ──

#[derive(Clone)]
pub(crate) struct VersionControlRuntime {
    /// 变更树导航的焦点句柄。
    focus: FocusHandle,
    /// 提交信息编辑器的独立焦点句柄。与导航分离，避免 `on_key_down` 拦截文本输入。
    commit_focus: FocusHandle,
    model: Rc<RefCell<VersionControlModel>>,
    /// Rc 包装确保 clone 后共享同一回调。
    click_callback: Rc<RefCell<Option<Rc<dyn Fn(PathBuf, &mut Window, &mut gpui::App)>>>>,
    /// 提交信息编辑器插槽。ShellRuntime 装配完成后通过 set_slot 注入。
    slot: Rc<RefCell<Option<Rc<TextEditorSlot>>>>,
}

impl VersionControlRuntime {
    pub(crate) fn new<T>(cx: &mut Context<T>, git_handle: Rc<RefCell<GitService>>) -> Self {
        Self {
            focus: cx.focus_handle(),
            commit_focus: cx.focus_handle(),
            model: Rc::new(RefCell::new(VersionControlModel::new(git_handle))),
            click_callback: Rc::new(RefCell::new(None)),
            slot: Rc::new(RefCell::new(None)),
        }
    }

    pub(crate) fn commit_focus_handle(&self) -> FocusHandle {
        self.commit_focus.clone()
    }

    pub(crate) fn set_click_callback(&self, cb: Rc<dyn Fn(PathBuf, &mut Window, &mut gpui::App)>) {
        *self.click_callback.borrow_mut() = Some(cb);
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn state(&self, has_project: bool) -> VersionControlState {
        self.model.borrow_mut().state(has_project)
    }

    pub(crate) fn move_selection(&self, delta: isize) {
        self.model.borrow_mut().move_selection(delta);
    }

    pub(crate) fn select(&self, path: PathBuf) {
        self.model.borrow_mut().selected = Some(path);
    }

    pub(crate) fn toggle_selected(&self) {
        self.model.borrow_mut().toggle_selected();
    }

    pub(crate) fn activate_selected(&self) -> Option<PathBuf> {
        self.model.borrow_mut().activate_selected()
    }

    pub(crate) fn toggle_stage(&self, path: &Path, window: &mut Window) {
        self.model.borrow_mut().toggle_stage(path, window);
    }

    pub(crate) fn collapse_or_parent(&self) {
        self.model.borrow_mut().collapse_or_parent();
    }

    pub(crate) fn expand_or_into(&self) {
        self.model.borrow_mut().expand_or_into();
    }

    pub(crate) fn set_slot(&self, slot: Rc<TextEditorSlot>) {
        *self.slot.borrow_mut() = Some(slot);
    }

    pub(crate) fn owner_handle(&self) -> Rc<RefCell<dyn TextTargetOwner>> {
        self.model.clone()
    }

    pub(crate) fn commit_text(&self) -> String {
        self.model.borrow().commit_text()
    }

    pub(crate) fn take_pending_bubbles(&self) -> Vec<BubbleRequest> {
        self.model.borrow_mut().take_pending_bubbles()
    }

    pub(crate) fn stage_all(&self, window: &mut Window) {
        let git = self.model.borrow().git_service.clone();
        if let Err(e) = git.borrow().stage_all() {
            self.model
                .borrow_mut()
                .pending_bubbles
                .push(BubbleRequest::error(format!("暂存全部失败：{e}")).dedupe("vc.stage_all"));
        }
        let _ = git.borrow_mut().refresh();
        self.model.borrow_mut().cached_generation = 0;
        window.refresh();
    }

    pub(crate) fn unstage_all(&self, window: &mut Window) {
        let git = self.model.borrow().git_service.clone();
        if let Err(e) = git.borrow().unstage_all() {
            self.model.borrow_mut().pending_bubbles.push(
                BubbleRequest::error(format!("取消暂存全部失败：{e}")).dedupe("vc.unstage_all"),
            );
        }
        let _ = git.borrow_mut().refresh();
        self.model.borrow_mut().cached_generation = 0;
        window.refresh();
    }

    /// 执行 git commit，成功时清空编辑区 + 弹气泡 + 切回导航焦点。
    pub(crate) fn perform_commit(&self, app: &Rc<RefCell<crate::app::App>>) {
        self.model.borrow_mut().try_commit();
        // 切回 Navigate 焦点。
        app.borrow_mut()
            .request_focus(AppFocus::Panel(PanelFocus::version_control()));
    }

    pub(crate) fn render(
        &self,
        key_request: &KeyRequest,
        titles: &CommandTitleLookup,
        has_project: bool,
    ) -> Div {
        let title = titles(COMMAND).unwrap_or_else(|| COMMAND.to_string());
        let state = self.state(has_project);

        if !state.has_project {
            return render_focus_host(
                &self.focus,
                key_request,
                placeholder(format!("{title}占位中")).into_any_element(),
            );
        }

        if !state.is_git_repo {
            return render_focus_host(
                &self.focus,
                key_request,
                placeholder("当前目录不是 Git 仓库").into_any_element(),
            );
        }

        if state.is_empty {
            return render_focus_host(
                &self.focus,
                key_request,
                placeholder("无变更文件").into_any_element(),
            );
        }

        let selected = state.selected.clone();
        let runtime = self.clone();
        let checkbox_runtime = self.clone();
        let slot_opt = self.slot.borrow().clone();
        let commit_text = self.commit_text();
        let show_placeholder = commit_text.trim().is_empty();
        let commit_runtime = self.clone();
        let scroll_handle = self.model.borrow().scroll_handle.clone();

        let tree_list = view::render_list(
            &state,
            selected,
            &scroll_handle,
            move |path, window, cx| {
                runtime.select(path.clone());
                if let Some(cb) = runtime.click_callback.borrow().as_ref() {
                    cb(path, window, cx);
                }
            },
            move |path, window, _cx| {
                checkbox_runtime.toggle_stage(&path, window);
            },
        );

        // 提交编辑器的独立焦点宿主——只包裹 track_focus + on_key_down，
        // 不加 .size_full()，让 flex 布局中的树列表正常获得剩余空间。
        let commit_focus = self.commit_focus.clone();
        let kr = Rc::clone(key_request);
        let commit_editor_host = div()
            .track_focus(&commit_focus)
            .tab_index(0)
            .on_key_down(move |event, window, cx| {
                if kr(normalized_chord(&event.keystroke), window, cx) {
                    cx.stop_propagation();
                }
            })
            .child(view::CommitEditor::render(
                slot_opt.as_ref(),
                show_placeholder,
                move |window, _cx| {
                    commit_runtime.model.borrow_mut().try_commit();
                    window.refresh();
                },
            ));

        let body: AnyElement = div()
            .flex()
            .flex_col()
            .size_full()
            .child(view::render_top_bar(state.diff_stats, state.all_staged, {
                let rt = self.clone();
                move |window: &mut Window, _cx: &mut gpui::App| {
                    if state.all_staged {
                        rt.unstage_all(window);
                    } else {
                        rt.stage_all(window);
                    }
                }
            }))
            .child(div().flex_1().overflow_hidden().child(tree_list))
            .child(commit_editor_host)
            .into_any_element();

        render_focus_host(&self.focus, key_request, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::process::Command as StdCommand;

    fn init_git_repo(name: &str) -> (PathBuf, Rc<RefCell<GitService>>) {
        let dir = std::env::temp_dir().join(format!("zom-vc-test-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        StdCommand::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&dir)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "user.email", "test@zom.local"])
            .current_dir(&dir)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "user.name", "zom-test"])
            .current_dir(&dir)
            .output()
            .unwrap();

        let git = Rc::new(RefCell::new(GitService::new(&dir)));
        (dir, git)
    }

    #[test]
    fn tree_structure_should_match_expected_depth_and_order() {
        // 构造目录结构:
        // root/
        //   a.txt (M)
        //   sub/
        //     b.txt (M)
        //     c.txt (M)
        let (root, git) = init_git_repo("terminal-mask");

        // 提交初始状态
        File::create(root.join("a.txt")).unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        File::create(root.join("sub/b.txt")).unwrap();
        File::create(root.join("sub/c.txt")).unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(&root)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(&root)
            .output()
            .unwrap();

        // 修改所有文件产生变更
        fs::write(root.join("a.txt"), b"modified").unwrap();
        fs::write(root.join("sub/b.txt"), b"modified").unwrap();
        fs::write(root.join("sub/c.txt"), b"modified").unwrap();

        git.borrow_mut().refresh().unwrap();

        let mut model = VersionControlModel::new(git);
        model.build_index();
        let rows = model.visible_rows();

        // 验证树结构：深度、排序（目录优先 + 字母序）、展开状态
        println!("=== visible_rows ===");
        for row in &rows {
            println!(
                "d={} name={} is_dir={} expanded={}",
                row.depth, row.name, row.is_dir, row.expanded
            );
        }

        // depth=0: root
        assert_eq!(rows[0].depth, 0);
        assert!(rows[0].is_dir);
        assert!(rows[0].expanded);

        // depth=1: 目录优先，sub 在前
        let depth1: Vec<_> = rows.iter().filter(|r| r.depth == 1).collect();
        assert_eq!(depth1.len(), 2);
        assert_eq!(depth1[0].name, "sub");
        assert_eq!(depth1[1].name, "a.txt");

        // depth=2: 字母序 b.txt 前, c.txt 后
        let depth2: Vec<_> = rows.iter().filter(|r| r.depth == 2).collect();
        assert_eq!(depth2.len(), 2);
        assert_eq!(depth2[0].name, "b.txt");
        assert_eq!(depth2[1].name, "c.txt");
    }
}
