//! 最近项目列表持久化。
//!
//! 数据存储在 `~/.zcv/recent_projects.json`，最多保留 20 条。

pub(crate) mod project_picker;

pub(crate) use project_picker::{
    OnProjectSelected, OpenLocalProject, ProjectPicker, ToggleProjectPicker,
};

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ═══ 数据 ════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct ProjectEntry {
    pub(crate) label: String,
    pub(crate) path: String,
    pub(crate) is_current: bool,
}

#[derive(Serialize, Deserialize)]
struct RecentProjects {
    recent: Vec<ProjectEntry>,
}

// ═══ 路径 ════════════════════════════════════════════════════════

fn config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".zcv")
}

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
        Ok(content) => serde_json::from_str::<RecentProjects>(&content)
            .map(|r| r.recent)
            .unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// 保存最近项目列表到磁盘。
pub(crate) fn save_recent_projects(projects: &[ProjectEntry]) {
    let dir = config_dir();
    let _ = std::fs::create_dir_all(&dir);
    let data = RecentProjects {
        recent: projects.to_vec(),
    };
    if let Ok(content) = serde_json::to_string_pretty(&data) {
        let _ = std::fs::write(recent_path(), content);
    }
}

/// 把一条路径添加到最近项目列表前端（去重），并标记为当前项目。
pub(crate) fn add_to_recent(path: &str) {
    let label = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string());

    let mut projects = load_recent_projects();
    // 移除旧记录
    projects.retain(|p| p.path != path);
    // 插入到最前
    projects.insert(
        0,
        ProjectEntry {
            label,
            path: path.to_string(),
            is_current: true,
        },
    );
    // 标记其余为非当前
    for p in projects.iter_mut().skip(1) {
        p.is_current = false;
    }
    projects.truncate(20);
    save_recent_projects(&projects);
}
