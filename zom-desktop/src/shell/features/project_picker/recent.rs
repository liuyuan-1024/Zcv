//! 最近项目的内存表与磁盘持久化。
//!
//! [`RecentProjects`] 持有"内存里的最近项目列表 + 它所在的磁盘路径"，
//! 任何修改（[`remember`] / [`remove`]）写完内存就立即写盘 —— 调用方不必
//! 再额外触发 flush。测试场景下传 `None` 路径即可拿到一份纯内存的实例。
//!
//! [`remember`]: RecentProjects::remember
//! [`remove`]: RecentProjects::remove

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::home_dir;
use serde::{Deserialize, Serialize};

/// 顶栏项目选择器使用的最近项目摘要。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RecentProject {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) identifier: String,
    pub(crate) repo: Option<String>,
}

/// 最近项目内存表 + 落盘路径。
#[derive(Default)]
pub(crate) struct RecentProjects {
    items: Vec<RecentProject>,
    path: Option<PathBuf>,
    /// 读 / 写 / 解析时产生的人类可读错误，调用方在合适时机 drain 出去显示给用户。
    pending_warnings: Vec<String>,
}

impl RecentProjects {
    /// 从磁盘加载；`None` 路径表示内存模式（测试用）。
    pub(crate) fn load(path: Option<PathBuf>) -> Self {
        let mut pending_warnings = Vec::new();
        let items = path
            .as_deref()
            .map(|p| read_recent_from_file(p, &mut pending_warnings))
            .unwrap_or_default();
        Self {
            items,
            path,
            pending_warnings,
        }
    }

    /// 取走累积的人类可读警告。
    pub(crate) fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_warnings)
    }

    /// 发行版默认落盘位置：`$HOME/.zom/recent_workspaces.toml`。
    pub(crate) fn default_path() -> Option<PathBuf> {
        Some(home_dir()?.join(".zom/recent_workspaces.toml"))
    }

    pub(crate) fn items(&self) -> &[RecentProject] {
        &self.items
    }

    /// 把一个项目记成"最近打开"。同 id 的旧记录会被去重；新条目永远在最前。
    pub(crate) fn remember(&mut self, root: PathBuf, repo: Option<String>) {
        let id = project_id(&root);
        self.items.retain(|project| project.id != id);
        self.items.insert(
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
        self.flush();
    }

    pub(crate) fn remove(&mut self, id: &str) {
        self.items.retain(|project| project.id != id);
        self.flush();
    }

    fn flush(&mut self) {
        let Some(path) = &self.path else {
            return;
        };
        if let Err(error) = write_recent_to_file(path, &self.items) {
            self.pending_warnings
                .push(format!("写入最近项目失败：{error}"));
        }
    }
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

fn project_id(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn project_name(path: &Path) -> Option<&str> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
}

fn read_recent_from_file(path: &Path, warnings: &mut Vec<String>) -> Vec<RecentProject> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            warnings.push(format!("读取最近项目失败：{error}"));
            return Vec::new();
        }
    };
    let file = match toml::from_str::<RecentProjectsFile>(&text) {
        Ok(file) => file,
        Err(error) => {
            warnings.push(format!("解析最近项目失败：{error}"));
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

fn write_recent_to_file(path: &Path, projects: &[RecentProject]) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    //! RecentProjects 的最近列表语义与磁盘持久化。
    //!
    //! 这些用例之前挂在 `App` 的 headless 单测里（迁移前 App 直接持有 RecentProjects）。
    //! 数据归属下沉到 picker runtime 之后，行为是纯 RecentProjects 的事——直接调
    //! 公共 API 验证，无需 App / GPUI。
    use super::*;
    use std::fs::{File, create_dir_all};

    fn project_fixture(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("zom-recent-projects-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_dir_all(dir.join("src")).unwrap();
        File::create(dir.join("README.md")).unwrap();
        dir
    }

    #[test]
    fn remember_should_dedupe_and_promote_to_front() {
        let mut recent = RecentProjects::load(None);
        let local = project_fixture("dedupe-local");
        let cloned = project_fixture("dedupe-git");

        recent.remember(local.clone(), None);
        recent.remember(
            cloned.clone(),
            Some("https://example.com/org/dedupe-git.git".to_string()),
        );

        let items = recent.items();
        assert_eq!(items.len(), 2);
        // 新条目永远在最前。
        assert_eq!(items[0].path, cloned);
        assert_eq!(
            items[0].identifier,
            "https://example.com/org/dedupe-git.git"
        );
        assert_eq!(items[1].path, local);
    }

    #[test]
    fn remove_should_drop_by_id() {
        let mut recent = RecentProjects::load(None);
        let local = project_fixture("remove-local");
        let cloned = project_fixture("remove-git");

        recent.remember(local.clone(), None);
        recent.remember(
            cloned.clone(),
            Some("https://example.com/org/remove-git.git".to_string()),
        );

        let id = recent.items()[0].id.clone();
        recent.remove(&id);

        let items = recent.items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].path, local);
    }

    #[test]
    fn remember_should_persist_to_file() {
        let store = std::env::temp_dir().join(format!(
            "zom-recent-projects-persist-{}.toml",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&store);
        let local = project_fixture("persist-local");
        let cloned = project_fixture("persist-git");

        {
            let mut recent = RecentProjects::load(Some(store.clone()));
            recent.remember(local.clone(), None);
            recent.remember(
                cloned.clone(),
                Some("https://example.com/org/persist-git.git".to_string()),
            );
        }

        // 重新 load 后内容仍在；顺序按"最近 → 最早"。
        let recent = RecentProjects::load(Some(store.clone()));
        let items = recent.items();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].path, cloned);
        assert_eq!(
            items[0].repo.as_deref(),
            Some("https://example.com/org/persist-git.git")
        );
        assert_eq!(items[1].path, local);

        let _ = std::fs::remove_file(store);
    }
}
