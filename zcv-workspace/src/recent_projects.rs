//! 最近项目列表持久化。
//!
//! 数据存储在统一配置目录（`~/.zcv`）的 `recent_projects.json`，最多保留 20 条。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zcv_settings::config_dir;

// ═══ 数据 ════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub label: String,
    pub path: String,
    pub is_current: bool,
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

/// 保存最近项目列表到磁盘。
pub fn save_recent_projects(projects: &[ProjectEntry]) {
    let dir = config_dir();
    let _ = std::fs::create_dir_all(&dir);
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

/// 把一条路径添加到最近项目列表前端（去重），并标记为当前项目。
pub fn add_to_recent(path: &str) {
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
