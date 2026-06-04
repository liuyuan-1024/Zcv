//! 共享 [`SyntaxEngine`] 的可嵌入多行编辑器目标。
//!
//! 自持的小输入框（[`super::OwnedEditorTarget`]）只需要 buffer + 选区——
//! 单行场景里没有「文件 / 语言」概念，语法高亮纯属浪费。**可嵌入**编辑器
//! （设置面板的 config.toml 视图、未来的代码片段输入框、行内 Markdown
//! 预览 ……）则希望与主编辑区共享同一套语言注册表与同一根后台 worker，
//! 落地为：底下托一个 [`SyntaxDocument`]，外面包一份选区与目标层胶水。
//!
//! 共享而非各开一份引擎：进程里**只**有主编辑区在 boot 期实例化的那根
//! [`SyntaxEngine`]，嵌入点构造时通过 `Rc` 借用。每个嵌入编辑器不再自己
//! 安装内置 provider，也不再各起一根后台线程。

use std::rc::Rc;

use zom_command::EditTarget;
use zom_engine::{ByteOffset, SelectionSet};
use zom_workspace::SyntaxDocument;
use zom_workspace::syntax::{LanguageId, SyntaxEngine};

use crate::shell::editor::highlight;
use crate::shell::editor::input::{ImeQueryTarget, ImeTarget};
use crate::shell::editor::snapshot::{EditorSnapshot, EditorSnapshotRequest, build_snapshot};

pub(crate) struct EmbeddedEditorTarget {
    document: SyntaxDocument,
    selection: SelectionSet,
}

impl EmbeddedEditorTarget {
    /// 构造一个语言已钉死的嵌入编辑器目标。
    /// `engine` 与主编辑区共享同一根 `Rc`——禁止在嵌入点新建独立引擎。
    /// 嵌入点对自己的语言通常已知（设置面板就是 toml），所以直接传 [`LanguageId`]，不再绕一圈伪造 path 让 registry detect。
    pub(crate) fn for_language(engine: Rc<SyntaxEngine>, language: LanguageId) -> Self {
        let document = SyntaxDocument::empty(engine, language).expect("空 Buffer 构造不会失败");
        Self {
            document,
            selection: SelectionSet::default(),
        }
    }

    /// 重置文档文本：清空旧高亮、依 `path` 重新识别语言、清空选区。
    pub(crate) fn replace_text(&mut self, text: &str) {
        self.document
            .replace_text(text.to_string())
            .expect("嵌入编辑器文本构造不会失败");
        self.selection = SelectionSet::default();
    }

    pub(crate) fn text(&self) -> String {
        let buffer = self.document.buffer();
        buffer
            .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
            .expect("嵌入编辑器文本范围来自自身长度")
            .into_text()
            .into_owned()
    }

    pub(crate) fn snapshot(&self, request: EditorSnapshotRequest) -> EditorSnapshot {
        let mut snapshot = build_snapshot(self.document.buffer(), &self.selection, request);
        highlight::push_syntax_layers(
            self.document.highlight_layers(),
            &snapshot.lines,
            &mut snapshot.decorations,
        );
        snapshot
    }

    /// 每次编辑后被 router 调用：把 pending DeltaEvent 喂给高亮 layer 与
    /// provider。对应 [`SyntaxDocument::pump_post_edit`]。
    pub(crate) fn pump_post_edit(&mut self) {
        let _ = self.document.pump_post_edit();
    }

    /// 每帧 prepaint 由 app 调一次：收割后台 worker 已就绪的高亮产物。
    /// 与主工作区的 `pump_pending_highlights` 同节奏。
    pub(crate) fn pump_pending_highlights(&mut self) {
        self.document.pump_pending_highlights();
    }

    pub(crate) fn as_edit_target(&mut self) -> EditTarget<'_> {
        EditTarget {
            buffer: self.document.buffer_mut(),
            selection: &mut self.selection,
            wrap_map: None,
            visual_caret: None,
            goal_column: None,
        }
    }

    pub(crate) fn as_ime_target(&mut self) -> ImeTarget<'_> {
        ImeTarget::new(self.document.buffer_mut(), &mut self.selection)
    }

    pub(crate) fn as_ime_query_target(&self) -> ImeQueryTarget<'_> {
        ImeQueryTarget::new(self.document.buffer(), &self.selection)
    }

    #[cfg(test)]
    pub(crate) fn wait_for_syntax_idle(&self) {
        self.document.engine().worker().wait_for_idle();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine_with_builtin_providers() -> Rc<SyntaxEngine> {
        let mut engine = SyntaxEngine::new();
        zom_workspace::syntax::install_builtin_providers(&mut engine);
        Rc::new(engine)
    }

    #[test]
    fn embedded_target_uses_shared_engine_syntax_provider() {
        let engine = engine_with_builtin_providers();
        let mut target = EmbeddedEditorTarget::for_language(engine, LanguageId::new("toml"));
        target.replace_text("[editor]\nsoft_wrap = false\n");
        target.wait_for_syntax_idle();
        target.pump_pending_highlights();

        let snapshot = target.snapshot(EditorSnapshotRequest::viewport(0, 2));

        assert!(
            snapshot.decorations.iter().any(|decoration| matches!(
                decoration.kind,
                crate::shell::editor::highlight::DecorationKind::Foreground
            )),
            "嵌入编辑器应能从共享 SyntaxEngine 拿到语法装饰"
        );
    }
}
