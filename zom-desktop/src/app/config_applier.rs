//! 把 [`AppConfig`] 推到 app runtime 下游：当前只负责 [`WorkspaceSession`] 的
//! buffer 配置。
//!
//! 无状态。所有 setter 幂等，不做 diff——重复调用 cost 可忽略，diff 既不省时也不防副作用。
//! 调用方拿 `store.config()` 一次性 push 全量。

use crate::config::AppConfig;
use crate::workspace_session::WorkspaceSession;

pub(super) struct ConfigApplier;

impl ConfigApplier {
    /// 把 config 的 [`BufferConfig`](zom_engine::BufferConfig) 推到当前
    /// [`WorkspaceSession`]。
    pub(super) fn apply_to_session(config: &AppConfig, session: &mut WorkspaceSession) {
        session
            .workspace_mut()
            .set_buffer_config(config.buffer_config());
    }
}
