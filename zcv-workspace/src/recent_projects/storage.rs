//! 最近项目列表持久化。
//!
//! 数据存储在统一配置目录（`~/.zcv`）的 `recent_projects.json`，最多保留 20 条。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};
use zcv_path::AbsolutePathBuf;
use zcv_settings::config_dir;

// ═══ 数据 ════════════════════════════════════════════════════════

/// 一条最近项目记录：一个工作区的有序根集合。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ProjectEntry {
    pub(crate) paths: Vec<String>,
}

impl ProjectEntry {
    /// 显示名：各根末段以顿号连接。
    pub(crate) fn label(&self) -> String {
        self.paths
            .iter()
            .map(|path| {
                Path::new(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join("、")
    }

    /// 过滤匹配文本：全部根路径拼接（与显示名一起参与模糊匹配）。
    pub(crate) fn search_text(&self) -> String {
        self.paths.join(" ")
    }
}

/// 读取格式：当前多根记录，以及旧版单根记录。
/// 旧记录读取时迁移为单元素集合；下次写盘即以新格式落盘，全部淘汰后可移除该分支。
#[derive(Deserialize)]
#[serde(untagged)]
enum ProjectEntryRepr {
    Current { paths: Vec<String> },
    Legacy { path: String },
}

impl<'de> Deserialize<'de> for ProjectEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match ProjectEntryRepr::deserialize(deserializer)? {
            ProjectEntryRepr::Current { paths } => Self { paths },
            ProjectEntryRepr::Legacy { path } => Self { paths: vec![path] },
        })
    }
}

#[derive(Serialize, Deserialize)]
struct RecentProjectsFile {
    recent: Vec<ProjectEntry>,
}

// ═══ 路径 ════════════════════════════════════════════════════════

fn recent_path() -> PathBuf {
    config_dir().join("recent_projects.json")
}

// ═══ 读写 ════════════════════════════════════════════════════════

/// 从磁盘加载最近项目列表。
pub(crate) fn load_recent_projects() -> Vec<ProjectEntry> {
    let path = recent_path();
    if !path.exists() {
        return Vec::new();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str::<RecentProjectsFile>(&content)
            .map(|r| r.recent)
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// 返回最近的仍有效工作区（至少一个根仍存在）。
///
/// 每个根必须是带名称的绝对目录。这样不会把从 `.app` 启动时的 `/` 或旧版本曾写入的相对路径 `.` 当成项目重新打开。
///
/// 失效记录只在此处读取时跳过，不清理写回；
/// 重新打开项目写盘时会以新格式覆盖，旧记录自然淘汰。
pub fn most_recent_valid_project() -> Option<Vec<PathBuf>> {
    load_recent_projects().iter().find_map(valid_project_paths)
}

/// 条目中仍有效的根集合；全部失效时条目视为失效。
fn valid_project_paths(entry: &ProjectEntry) -> Option<Vec<PathBuf>> {
    let paths = entry
        .paths
        .iter()
        .filter_map(|path| canonical_project_path(Path::new(path)))
        .collect::<Vec<_>>();
    (!paths.is_empty()).then_some(paths)
}

/// 项目根必须是带名称的绝对目录（canonicalize 失败即视为失效）。
fn canonical_project_path(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let path = AbsolutePathBuf::canonicalize(path).ok()?.into_path_buf();
    (path.is_dir() && path.file_name().is_some()).then_some(path)
}

/// 保存最近项目列表到磁盘（原子写盘，失败返回错误）。
pub(crate) fn save_recent_projects(projects: &[ProjectEntry]) -> anyhow::Result<()> {
    let data = RecentProjectsFile {
        recent: projects.to_vec(),
    };
    let content = serde_json::to_vec_pretty(&data)
        .map_err(|error| anyhow::anyhow!("序列化最近项目列表失败：{error}"))?;
    crate::persistence::atomic_write(&recent_path(), &content)
}

/// 从最近项目列表移除一条工作区记录（幂等，不存在时无副作用）。
pub fn remove_from_recent(paths: &[String]) -> anyhow::Result<()> {
    let mut projects = load_recent_projects();
    let before = projects.len();
    projects.retain(|p| p.paths != paths);
    // 列表未变化时不写盘，避免无谓 IO
    if projects.len() == before {
        return Ok(());
    }
    save_recent_projects(&projects)
}

/// 把一个工作区（有序根集合）添加到最近项目列表前端（去重，首位即最近打开的工作区）。
///
/// 全部根失效时不记录；部分失效时只记录仍有效的根。
/// 不同根集合是不同的工作区，各自成条目；同一工作区变根后的旧条目由调用方移除。
pub fn add_to_recent(paths: &[PathBuf]) -> anyhow::Result<()> {
    let paths = paths
        .iter()
        .filter_map(|path| canonical_project_path(path))
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return Ok(());
    }

    let mut projects = load_recent_projects();
    // 已在首位时不动，避免无谓写盘
    if projects.first().is_some_and(|p| p.paths == paths) {
        return Ok(());
    }
    projects.retain(|p| p.paths != paths);
    projects.insert(0, ProjectEntry { paths });
    projects.truncate(20);
    save_recent_projects(&projects)
}

#[cfg(test)]
#[path = "../test/recent_projects_storage_tests.rs"]
mod tests;
