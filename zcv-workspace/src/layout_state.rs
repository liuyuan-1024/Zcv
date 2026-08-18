//! 工作区布局状态的轻量持久化后端。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use zcv_settings::config_dir;

use crate::dock::DockStructure;

const LAYOUT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceLayout {
    version: u32,
    docks: DockStructure,
}

pub(crate) fn path_for_workspace(root: Option<&Path>) -> PathBuf {
    let identity = root
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "__empty__".to_owned());
    // 固定 FNV-1a，避免依赖 DefaultHasher 的跨版本实现细节。
    let hash = identity.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    config_dir()
        .join("workspaces")
        .join(format!("{hash:016x}.json"))
}

pub(crate) fn load(path: &Path) -> Option<DockStructure> {
    let content = fs::read_to_string(path).ok()?;
    let layout: WorkspaceLayout = serde_json::from_str(&content).ok()?;
    (layout.version == LAYOUT_VERSION).then_some(layout.docks)
}

pub(crate) fn save(path: &Path, docks: DockStructure) -> Result<()> {
    let parent = path.parent().context("布局状态路径没有父目录")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("无法创建布局状态目录 {}", parent.display()))?;
    let content = serde_json::to_vec_pretty(&WorkspaceLayout {
        version: LAYOUT_VERSION,
        docks,
    })?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, content)
        .with_context(|| format!("无法写入临时布局状态 {}", temporary.display()))?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("无法替换旧布局状态 {}", path.display()))?;
    }
    fs::rename(&temporary, path).with_context(|| format!("无法提交布局状态 {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::*;
    use crate::dock::DockData;

    #[test]
    fn layout_round_trip_and_version_guard() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("layout.json");
        let docks = DockStructure {
            left: DockData {
                visible: true,
                active_panel: Some("project-tree".into()),
                size: Some(f32::from(px(320.0))),
            },
            ..DockStructure::default()
        };
        save(&path, docks.clone()).unwrap();
        assert_eq!(load(&path), Some(docks));

        fs::write(&path, r#"{"version":999,"docks":{}}"#).unwrap();
        assert_eq!(load(&path), None);
    }
}
