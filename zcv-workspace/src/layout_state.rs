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

pub(crate) const LAYOUT_VERSION: u32 = 4;

/// 可持久化的 Pane 标签类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SerializedPaneItem {
    Source(PathBuf),
    Preview(PathBuf),
    StandalonePreview(PathBuf),
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
            Self::Source(path) | Self::Preview(path) | Self::StandalonePreview(path) => Some(path),
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
#[path = "test/layout_state_tests.rs"]
mod tests;
