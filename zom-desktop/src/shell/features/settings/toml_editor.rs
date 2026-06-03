//! 设置面板的 config.toml 视图——一个挂着 toml 语法高亮的多行嵌入式编辑器。
//!
//! 把「读盘 / 兜底 / 解析 / 文本驻留 / target 路由钩子」全收口在本模块。
//! 组合根（[`crate::app::App`]）只调本类型的高层入口，不再亲自做 I/O 或
//! TOML 解析。

use std::path::Path;
use std::rc::Rc;

use zom_command::{EditTarget, KeyContext};
use zom_workspace::syntax::{LanguageId, SyntaxEngine};

use crate::config::AppConfig;
use crate::focus::{AppFocus, SurfaceFocus};
use crate::shell::editor::{
    EditorSnapshot, EditorSnapshotRequest, EmbeddedEditorTarget, ImeQueryTarget, ImeTarget,
    TextTargetOwner, TextTargetQuery,
};

pub(crate) struct SettingsTomlEditor {
    open: bool,
    target: EmbeddedEditorTarget,
}

impl SettingsTomlEditor {
    pub(crate) fn new(engine: Rc<SyntaxEngine>) -> Self {
        Self {
            open: false,
            target: EmbeddedEditorTarget::for_language(engine, LanguageId::new("toml")),
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    /// 主入口：从 `path` 读 config.toml；读不到（首次启动、文件被删等）
    /// 就退到把当前内存配置序列化的文本，让用户在编辑器里也能开始改。
    pub(crate) fn open_from_disk(&mut self, path: &Path, fallback: &AppConfig) {
        let text = std::fs::read_to_string(path)
            .ok()
            .unwrap_or_else(|| fallback.to_toml_string().unwrap_or_default());
        self.open_with_text(text);
    }

    /// 直接以已给定的文本打开——单测与「外部已读好 TOML 想直接灌进编辑器」
    /// 的 caller 走它。
    pub(crate) fn open_with_text(&mut self, text: impl Into<String>) {
        self.open = true;
        self.target.replace_text(text.into().as_str());
    }

    /// 关闭编辑器并把当前文本解析 + 归一为 [`AppConfig`]。解析失败时保留
    /// 打开态并向 stderr 打印诊断，返回 `None`，让用户在原文上继续修。
    pub(crate) fn close_and_parse(&mut self) -> Option<AppConfig> {
        let text = self.target.text();
        match toml::from_str::<AppConfig>(&text) {
            Ok(config) => {
                self.open = false;
                Some(config.normalized())
            }
            Err(error) => {
                eprintln!("解析设置 TOML 失败：{error}");
                None
            }
        }
    }

    /// 每帧 prepaint 由 [`crate::app::App::pump_pending_highlights`] 调一次。
    pub(crate) fn pump_pending_highlights(&mut self) {
        self.target.pump_pending_highlights();
    }

    #[cfg(test)]
    pub(crate) fn target_mut(&mut self) -> &mut EmbeddedEditorTarget {
        &mut self.target
    }
}

impl TextTargetQuery for SettingsTomlEditor {
    fn accepts_focus(&self, focus: AppFocus) -> bool {
        self.open && matches!(focus, AppFocus::Surface(SurfaceFocus::Settings))
    }

    fn snapshot(&self) -> EditorSnapshot {
        self.target
            .snapshot(EditorSnapshotRequest::viewport(0, 256))
    }

    fn key_contexts(&self) -> Vec<KeyContext> {
        vec![
            KeyContext::settings(),
            KeyContext::text_edit(self.accepts_newline(), false),
            KeyContext::global(),
        ]
    }

    fn accepts_newline(&self) -> bool {
        true
    }

    fn ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
        if !self.open {
            return None;
        }
        Some(self.target.as_ime_query_target())
    }
}

impl TextTargetOwner for SettingsTomlEditor {
    fn ime_target(&mut self) -> Option<ImeTarget<'_>> {
        if !self.open {
            return None;
        }
        Some(self.target.as_ime_target())
    }

    fn edit_target(&mut self) -> Option<EditTarget<'_>> {
        if !self.open {
            return None;
        }
        Some(self.target.as_edit_target())
    }

    fn after_text_changed(&mut self) {
        // EmbeddedEditorTarget 内部把 pending DeltaEvent 喂给高亮 layer 与 provider
        self.target.pump_post_edit();
    }
}
