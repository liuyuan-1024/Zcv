//! shell —— GPUI 外壳。
//!
//! 职责（手册 1.2、2.4）：
//! - 视觉层（L1-L4 组件树）
//! - 平台层
//! - 启动窗口、装配 `WorkbenchFrame`、提供 `EmbeddedAssetSource`
//!
//! `shell` 只在启动与根视图装配处接收 `app`；features / workbench 通过明确
//! 的 shared 原语和 feature API 协作。

mod boot;
pub(crate) mod bubble;
pub(crate) mod editor;
pub(crate) mod features;
pub(crate) mod platform;
pub(crate) mod project_session;
pub(crate) mod shared;
pub(crate) mod surfaces;
mod view;
pub(crate) mod workbench;

pub use boot::run;
pub(crate) use shared::interaction::{
    ActionRequest, CommandCatalogItem, CommandCatalogLookup, CommandTitleLookup, KeyRequest,
    ShortcutLookup,
};
pub(crate) use shared::keyboard::normalized_chord;
