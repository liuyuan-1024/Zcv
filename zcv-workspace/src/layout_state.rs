//! 工作区布局状态的轻量持久化后端。
//!
//! 键控与写盘复用 persistence 共享原语，与窗口边界保持同一项目身份。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use zcv_settings::config_dir;

use crate::dock::DockStructure;
use crate::persistence;

const LAYOUT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceLayout {
    version: u32,
    docks: DockStructure,
}

pub(crate) fn path_for_workspace(root: Option<&Path>) -> PathBuf {
    config_dir()
        .join("workspaces")
        .join(format!("{}.json", persistence::workspace_identity(root)))
}

pub(crate) fn load(path: &Path) -> Option<DockStructure> {
    let content = fs::read_to_string(path).ok()?;
    let layout: WorkspaceLayout = serde_json::from_str(&content).ok()?;
    (layout.version == LAYOUT_VERSION).then_some(layout.docks)
}

pub(crate) fn save(path: &Path, docks: DockStructure) -> Result<()> {
    let content = serde_json::to_vec_pretty(&WorkspaceLayout {
        version: LAYOUT_VERSION,
        docks,
    })?;
    persistence::atomic_write(path, &content)
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
