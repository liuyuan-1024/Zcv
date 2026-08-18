//! 最近项目列表持久化。
//!
//! 数据存储在统一配置目录（`~/.zcv`）的 `recent_projects.json`，最多保留 20 条。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zcv_settings::config_dir;

// ═══ 数据 ════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub path: String,
}

impl ProjectEntry {
    /// 显示名：从规范化的绝对路径取末段。
    pub fn label(&self) -> String {
        Path::new(&self.path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    }
}

#[derive(Serialize, Deserialize)]
struct RecentProjects {
    recent: Vec<ProjectEntry>,
}

// ═══ 路径 ════════════════════════════════════════════════════════

fn recent_path() -> PathBuf {
    config_dir().join("recent_projects.json")
}

// ═══ 读写 ════════════════════════════════════════════════════════

/// 从磁盘加载最近项目列表。
pub fn load_recent_projects() -> Vec<ProjectEntry> {
    let path = recent_path();
    if !path.exists() {
        return Vec::new();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str::<RecentProjects>(&content)
            .map(|r| r.recent)
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// 返回最近的有效项目。
///
/// 最近项目必须是带名称的绝对目录。这样不会把从 `.app` 启动时的 `/` 或旧版本曾写入的相对路径 `.` 当成项目重新打开。
///
/// 失效记录只在此处读取时跳过，不清理写回（对齐 Zed 读取时过滤失效路径的做法；
/// 重新打开项目写盘时会以新格式覆盖，旧记录自然淘汰）。
pub fn most_recent_valid_project() -> Option<PathBuf> {
    first_valid_project(&load_recent_projects())
}

/// 跳过失效记录，返回第一个仍有效的项目路径。
fn first_valid_project(projects: &[ProjectEntry]) -> Option<PathBuf> {
    projects
        .iter()
        .find_map(|project| canonical_project_path(Path::new(&project.path)))
}

/// 项目根必须是带名称的绝对目录（canonicalize 失败即视为失效）。
pub fn canonical_project_path(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let path = path.canonicalize().ok()?;
    (path.is_dir() && path.file_name().is_some()).then_some(path)
}

/// 保存最近项目列表到磁盘。
pub fn save_recent_projects(projects: &[ProjectEntry]) {
    let dir = config_dir();
    let _ = std::fs::create_dir_all(dir);
    let data = RecentProjects {
        recent: projects.to_vec(),
    };
    if let Ok(content) = serde_json::to_string_pretty(&data) {
        let _ = std::fs::write(recent_path(), content);
    }
}

/// 从最近项目列表移除一条路径（幂等，不存在时无副作用）。
pub fn remove_from_recent(path: &str) {
    let mut projects = load_recent_projects();
    let before = projects.len();
    projects.retain(|p| p.path != path);
    // 列表未变化时不写盘，避免无谓 IO
    if projects.len() != before {
        save_recent_projects(&projects);
    }
}

/// 把一条路径添加到最近项目列表前端（去重，首位即最近打开的项目）。
pub fn add_to_recent(path: &str) {
    let Some(path) = canonical_project_path(Path::new(path)) else {
        return;
    };
    let path = path.to_string_lossy().to_string();

    let mut projects = load_recent_projects();
    // 已在首位时不动，避免无谓写盘
    if projects.first().is_some_and(|p| p.path == path) {
        return;
    }
    projects.retain(|p| p.path != path);
    projects.insert(0, ProjectEntry { path });
    projects.truncate(20);
    save_recent_projects(&projects);
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{ProjectEntry, canonical_project_path, first_valid_project};

    #[test]
    fn recent_project_rejects_root_relative_and_missing_paths() {
        assert!(canonical_project_path(Path::new("/")).is_none());
        assert!(canonical_project_path(Path::new(".")).is_none());
        assert!(canonical_project_path(Path::new("/definitely/missing/zcv-project")).is_none());
    }

    #[test]
    fn recent_project_canonicalizes_parent_components() {
        let current = std::env::current_dir()
            .expect("应有当前目录")
            .canonicalize()
            .expect("当前目录应可规范化");
        let with_parent = current
            .join("..")
            .join(current.file_name().expect("当前目录应有名称"));
        assert_eq!(canonical_project_path(&with_parent), Some(current));
    }

    #[test]
    fn first_valid_project_skips_stale_entries() {
        let current = std::env::current_dir().expect("应有当前目录");
        let entries = vec![
            ProjectEntry {
                path: "/definitely/missing/zcv-project".into(),
            },
            ProjectEntry {
                path: current.to_string_lossy().to_string(),
            },
        ];
        assert_eq!(first_valid_project(&entries), Some(current));
    }

    #[test]
    fn first_valid_project_returns_none_when_all_stale() {
        let entries = vec![
            ProjectEntry {
                path: "/definitely/missing/a".into(),
            },
            ProjectEntry {
                path: "/definitely/missing/b".into(),
            },
        ];
        assert_eq!(first_valid_project(&entries), None);
    }
}
