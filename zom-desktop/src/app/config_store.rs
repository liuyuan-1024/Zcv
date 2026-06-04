//! 配置存储：[`AppConfig`] 的进程内副本、落盘路径、以及它向 editor kernel 投影出去的运行时只读 cell。
//!
//! Store 只负责"持有 + 落盘 + 保持 soft_wrap cell 与 config 一致"。
//! workspace buffer_config 的应用走 [`ConfigApplier`](super::config_applier::ConfigApplier)，
//! 视觉字段由 shell 在装配 / settings 变更后投影到主题系统。
//! Store 不依赖 workspace、不依赖 typography / syntax；测试它的 mutation 不需要 stub 任何全局子系统。

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::config::{AppConfig, SettingsChange};

pub(super) struct ConfigStore {
    config: AppConfig,
    config_path: Option<PathBuf>,
    /// 软换行的运行时只读投影：多行 [`EditorKernel`] 构造时借这份 `Rc`，
    /// 一次写入对所有持有者同帧可见。Store 是唯一的写入者，所有 mutate 路径
    /// 必须经 [`sync_soft_wrap_cell`](Self::sync_soft_wrap_cell) 保持一致。
    ///
    /// [`EditorKernel`]: crate::shell::editor::EditorKernel
    soft_wrap_state: Rc<Cell<bool>>,
}

impl ConfigStore {
    pub(super) fn new(config_path: Option<PathBuf>) -> Self {
        let config = AppConfig::load(config_path.as_deref());
        let soft_wrap_state = Rc::new(Cell::new(config.editor.soft_wrap));
        Self {
            config,
            config_path,
            soft_wrap_state,
        }
    }

    /// 借出 config 的只读引用——Applier 通过此入口拿到 push 给下游的源数据。
    pub(super) fn config(&self) -> &AppConfig {
        &self.config
    }

    pub(super) fn snapshot(&self) -> AppConfig {
        self.config.clone()
    }

    pub(super) fn path(&self) -> Option<PathBuf> {
        self.config_path.clone()
    }

    pub(super) fn soft_wrap_handle(&self) -> Rc<Cell<bool>> {
        self.soft_wrap_state.clone()
    }

    pub(super) fn buffer_config(&self) -> zom_engine::BufferConfig {
        self.config.buffer_config()
    }

    pub(super) fn save(&self) {
        self.config.save(self.config_path.as_deref());
    }

    /// 翻转软换行。视觉与 workspace 不受影响——所以本路径不需要 Applier 介入，
    /// 自己同步 cell 后直接落盘。
    pub(super) fn toggle_soft_wrap(&mut self) {
        let next = !self.soft_wrap_state.get();
        self.soft_wrap_state.set(next);
        self.config.editor.soft_wrap = next;
        self.save();
    }

    /// 应用一项 [`SettingsChange`]，保持 soft_wrap cell 与 config 一致。
    /// workspace 同步 / 落盘由调用方分别接 [`ConfigApplier::apply_to_session`]
    /// 与 [`Self::save`]。
    ///
    /// [`ConfigApplier::apply_to_session`]: super::config_applier::ConfigApplier::apply_to_session
    pub(super) fn apply_change(&mut self, change: SettingsChange) {
        self.config.apply_change(change);
        self.sync_soft_wrap_cell();
    }

    /// 整份替换 config，保持 soft_wrap cell 一致。视觉 / workspace / 落盘
    /// 同 [`apply_change`](Self::apply_change)。
    pub(super) fn replace(&mut self, next: AppConfig) {
        self.config = next;
        self.sync_soft_wrap_cell();
    }

    fn sync_soft_wrap_cell(&self) {
        self.soft_wrap_state.set(self.config.editor.soft_wrap);
    }
}
