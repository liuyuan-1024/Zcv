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

pub(crate) const LAYOUT_VERSION: u32 = 3;

/// 可持久化的 Pane 标签类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SerializedPaneItem {
    Source(PathBuf),
    Preview(PathBuf),
    /// 由具体 Item 重新构建的非文件标签。
    Custom {
        kind: String,
        state: serde_json::Value,
    },
}

impl SerializedPaneItem {
    /// 文件标签的路径；非文件标签没有可替代的单一路径。
    pub(crate) fn path(&self) -> Option<&Path> {
        match self {
            Self::Source(path) | Self::Preview(path) => Some(path),
            Self::Custom { .. } => None,
        }
    }
}

/// 中心 Pane 的固定标签快照；临时标签不写入布局。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SerializedPane {
    pub(crate) items: Vec<SerializedPaneItem>,
    pub(crate) active_item: Option<usize>,
}

/// 面板自持状态（面板经 `Panel::serialized_state` 提供，如终端会话列表）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct PanelState {
    /// 面板持久化标识（persistent_name）。
    pub(crate) name: String,
    /// 面板自定义序列化数据。
    pub(crate) data: serde_json::Value,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct WorkspaceLayout {
    pub(crate) version: u32,
    pub(crate) docks: DockStructure,
    pub(crate) pane: SerializedPane,
    pub(crate) panels: Vec<PanelState>,
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
                items: vec![
                    SerializedPaneItem::Source(PathBuf::from("a.txt")),
                    SerializedPaneItem::Preview(PathBuf::from("b.txt")),
                    SerializedPaneItem::Custom {
                        kind: "project-diff".into(),
                        state: serde_json::json!({ "kind": "staged" }),
                    },
                ],
                active_item: Some(2),
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
