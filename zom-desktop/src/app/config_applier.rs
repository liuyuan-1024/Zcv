//! 把 [`AppConfig`] 推到运行时的下游：全局视觉子系统（typography / syntax）
//! 与 [`WorkspaceSession`] 的 buffer 配置。
//!
//! 无状态。所有 setter 幂等，不做 diff——重复调用 cost 可忽略，diff 既不省时也不防副作用。
//! 调用方拿 `store.config()` 一次性 push 全量。

use crate::config::AppConfig;
use crate::shell::shared::theme::{syntax, typography};
use crate::workspace_session::WorkspaceSession;

pub(super) struct ConfigApplier;

impl ConfigApplier {
    /// 把字号 / 主题刷到全局视觉子系统。boot 期视觉初始化与运行期 mutate
    /// 后都调本入口。
    pub(super) fn apply_visuals(config: &AppConfig) {
        typography::set_sizes(config.ui.font_size, config.editor.font_size);
        syntax::set_theme(&config.general.theme);
    }

    /// 把 config 的 [`BufferConfig`](zom_engine::BufferConfig) 推到当前
    /// [`WorkspaceSession`]。
    pub(super) fn apply_to_session(config: &AppConfig, session: &mut WorkspaceSession) {
        session
            .workspace_mut()
            .set_buffer_config(config.buffer_config());
    }

    /// 视觉 + workspace 全量应用。运行期 mutate（apply_change / replace）后
    /// 由调用方喊一次；boot 期不走这一路，因为那时 session 还未构造。
    pub(super) fn apply_all(config: &AppConfig, session: &mut WorkspaceSession) {
        Self::apply_visuals(config);
        Self::apply_to_session(config, session);
    }
}
