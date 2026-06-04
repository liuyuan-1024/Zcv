//! 配置运行时。
//!
//! `AppConfig` 是持久化数据；
//! 本运行时保存它的进程内副本、落盘路径，以及需要被编辑器 kernel 共享的运行时开关。

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::config::{AppConfig, SettingsChange};
use crate::workspace_session::WorkspaceSession;

pub(crate) struct ConfigRuntime {
    config: AppConfig,
    config_path: Option<PathBuf>,
    soft_wrap_state: Rc<Cell<bool>>,
}

impl ConfigRuntime {
    pub(crate) fn new(config_path: Option<PathBuf>) -> Self {
        let config = AppConfig::load(config_path.as_deref());
        config.apply_runtime_visuals();
        let soft_wrap_state = Rc::new(Cell::new(config.editor.soft_wrap));
        Self {
            config,
            config_path,
            soft_wrap_state,
        }
    }

    pub(crate) fn snapshot(&self) -> AppConfig {
        self.config.clone()
    }

    pub(crate) fn path(&self) -> Option<PathBuf> {
        self.config_path.clone()
    }

    pub(crate) fn soft_wrap_handle(&self) -> Rc<Cell<bool>> {
        self.soft_wrap_state.clone()
    }

    pub(crate) fn buffer_config(&self) -> zom_engine::BufferConfig {
        self.config.buffer_config()
    }

    pub(crate) fn save(&self) {
        self.config.save(self.config_path.as_deref());
    }

    pub(crate) fn toggle_soft_wrap(&mut self) {
        let next = !self.soft_wrap_state.get();
        self.soft_wrap_state.set(next);
        self.config.editor.soft_wrap = next;
        self.save();
    }

    pub(crate) fn apply_settings_change(
        &mut self,
        change: SettingsChange,
        session: &mut WorkspaceSession,
    ) {
        self.config.apply_change(change);
        self.apply_runtime_config(session);
        self.save();
    }

    pub(crate) fn replace_config(&mut self, next: AppConfig, session: &mut WorkspaceSession) {
        self.config = next;
        self.apply_runtime_config(session);
        self.save();
    }

    pub(crate) fn apply_runtime_config(&mut self, session: &mut WorkspaceSession) {
        self.config.apply_runtime_visuals();
        session
            .workspace_mut()
            .set_buffer_config(self.config.buffer_config());
        self.soft_wrap_state.set(self.config.editor.soft_wrap);
    }
}
