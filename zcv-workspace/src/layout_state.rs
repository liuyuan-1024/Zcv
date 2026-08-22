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

pub(crate) const LAYOUT_VERSION: u32 = 2;

/// 中心 Pane 的标签快照：文件路径按 tab 顺序排列，active_item 为活动标签索引。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SerializedPane {
    pub items: Vec<PathBuf>,
    pub active_item: Option<usize>,
}

/// 面板自持状态（面板经 `Panel::serialized_state` 提供，如终端会话列表）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PanelState {
    /// 面板持久化标识（persistent_name）。
    pub name: String,
    /// 面板自定义序列化数据。
    pub data: serde_json::Value,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceLayout {
    pub version: u32,
    pub docks: DockStructure,
    pub pane: SerializedPane,
    pub panels: Vec<PanelState>,
}

pub(crate) fn path_for_workspace(root: Option<&Path>) -> PathBuf {
    config_dir()
        .join("workspaces")
        .join(format!("{}.json", persistence::workspace_identity(root)))
}

pub(crate) fn load(path: &Path) -> Option<WorkspaceLayout> {
    let content = fs::read_to_string(path).ok()?;
    let layout: WorkspaceLayout = serde_json::from_str(&content).ok()?;
    (layout.version == LAYOUT_VERSION).then_some(layout)
}

pub(crate) fn save(path: &Path, layout: &WorkspaceLayout) -> Result<()> {
    let content = serde_json::to_vec_pretty(layout)?;
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
        let layout = WorkspaceLayout {
            version: LAYOUT_VERSION,
            docks: DockStructure {
                left: DockData {
                    visible: true,
                    active_panel: Some("project-tree".into()),
                    size: Some(f32::from(px(320.0))),
                },
                ..DockStructure::default()
            },
            pane: SerializedPane {
                items: vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")],
                active_item: Some(1),
            },
            panels: Vec::new(),
        };
        save(&path, &layout).unwrap();
        assert_eq!(load(&path), Some(layout));

        // 版本不匹配（旧版或未知版本）一律不加载，回到全新默认布局。
        fs::write(
            &path,
            r#"{"version":1,"docks":{"left":{"visible":true,"active_panel":"project-tree","size":320.0},"right":{"visible":false},"bottom":{"visible":false}},"pane":{"items":[],"active_item":null},"panels":[]}"#,
        )
        .unwrap();
        assert_eq!(load(&path), None);

        fs::write(&path, r#"{"version":999,"docks":{}}"#).unwrap();
        assert_eq!(load(&path), None);
    }
}
