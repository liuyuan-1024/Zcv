//! `SyntaxDocument` —— 单缓冲区文档。
//!
//! 嵌入式编辑器（设置面板的 TOML 视图、未来的代码片段输入框 ……）需要的不是一整个 [`crate::Workspace`] —— 它们没有多 buffer / active buffer / 文件树这些概念，只是想要「一个挂着语法高亮的 [`Buffer`]」。
//!
//! 本类型把缓冲区生命周期与语法高亮调度内聚成一个值：
//!
//! - 持有 [`Buffer`] + 可选的 [`BufferSyntax`] —— 与 [`crate::WorkspaceBuffer`] 同款。
//! - 通过 `Rc<SyntaxEngine>` 与主工作区共享语言注册表 / 后台 worker / buffer id 分配器。
//! - 暴露 [`Self::pump_post_edit`] 把编辑事件喂给 syntax；paint 端按 [`Self::syntax_tree_slot`] 现查共享 tree。
//!
//! ## 为什么直接吃 [`LanguageId`]
//!
//! 嵌入式编辑器对自身语言通常**已知**（设置面板就是 toml，代码片段输入框由调用方决定），
//! 不需要走 [`crate::Workspace::open_*`] 那种「按 path + 首行 shebang 识别」的路径。直接传 [`LanguageId`] 让 API 表达意图。

use std::rc::Rc;

use zom_engine::{Buffer, BufferConfig, DeltaEvent};

use crate::syntax::{
    BufferSyntax, BufferSyntaxTreeSlot, LanguageId, MAX_HIGHLIGHT_BYTES, SyntaxEngine,
};
use crate::{BufferId, WorkspaceResult};

/// 单个挂着语法高亮的缓冲区。
#[derive(Debug)]
pub struct SyntaxDocument {
    engine: Rc<SyntaxEngine>,
    buffer_id: BufferId,
    buffer: Buffer,
    /// 语言钉死在构造期：嵌入式编辑器自知语言，不走 path 识别。
    /// `LanguageId::PLAIN` 表示纯文本编辑（不挂 provider）。
    language: LanguageId,
    syntax: Option<BufferSyntax>,
}

impl SyntaxDocument {
    /// 接管一个已经造好的 [`Buffer`]——主工作区路径（`Workspace::open_file` 走流式 decoder、`Workspace::open_text` 走 `from_text`，buffer 都在外面造好）从这条入口构造文档。
    /// 构造时按 `language` 立刻 attach provider。
    pub fn from_buffer(engine: Rc<SyntaxEngine>, buffer: Buffer, language: LanguageId) -> Self {
        let buffer_id = engine.allocate_buffer_id();
        let mut doc = Self {
            engine,
            buffer_id,
            buffer,
            language,
            syntax: None,
        };
        doc.attach_syntax();
        doc
    }

    /// 空文档：buffer 没有内容、provider 尚未 attach——首次 [`Self::replace_text`] 才触发 attach。
    /// 嵌入式编辑器（构造期还不知道初始文本）走它。
    pub fn empty(engine: Rc<SyntaxEngine>, language: LanguageId) -> WorkspaceResult<Self> {
        let buffer = Buffer::new(BufferConfig::default())?;
        let buffer_id = engine.allocate_buffer_id();
        Ok(Self {
            engine,
            buffer_id,
            buffer,
            language,
            syntax: None,
        })
    }

    /// 文档初始即带内容：常用于「打开磁盘文本投影到嵌入编辑器」。
    /// 内部走 [`Self::from_buffer`]，构造期就 attach。
    pub fn with_text(
        engine: Rc<SyntaxEngine>,
        language: LanguageId,
        text: impl Into<String>,
    ) -> WorkspaceResult<Self> {
        let buffer = Buffer::from_text(text.into(), BufferConfig::default())?;
        Ok(Self::from_buffer(engine, buffer, language))
    }

    /// 文档持有的稳定 [`BufferId`]。
    pub fn buffer_id(&self) -> BufferId {
        self.buffer_id
    }

    /// 重置文档内容；语言保持不变，只重挂 provider。
    ///
    /// 不变量：旧 syntax 在新文本灌入前 detach（slot 清空）；新文本灌入后再 attach。
    pub fn replace_text(&mut self, text: impl Into<String>) -> WorkspaceResult<()> {
        if let Some(state) = self.syntax.take() {
            state.detach();
        }
        self.buffer = Buffer::from_text(text.into(), self.buffer.config().clone())?;
        self.attach_syntax();
        Ok(())
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    pub fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }

    /// 共享的 [`SyntaxEngine`]——同进程其他嵌入点也可借同一根 `Rc`。
    pub fn engine(&self) -> &Rc<SyntaxEngine> {
        &self.engine
    }

    /// 当前缓冲区的共享 [`BufferSyntaxTreeSlot`]。
    ///
    /// `None` 表示 buffer 未挂 provider（plain 语言 / 超阈值 / 注册表缺工厂）。
    /// 调用方 `load()` 拿到 `Arc<BufferSyntaxTree>` 后按 viewport 现查 Query。
    pub fn syntax_tree_slot(&self) -> Option<&BufferSyntaxTreeSlot> {
        self.syntax.as_ref().map(|s| s.tree_slot())
    }

    /// 构造期钉死的语言；`LanguageId::PLAIN` 表示不挂 provider 的纯文本。
    pub fn language(&self) -> LanguageId {
        self.language
    }

    /// 编辑后调一次：排空 buffer pending events，喂给 syntax。
    ///
    /// 嵌入式文档不挂 `BufferSearch`，所以这条路径**独自**消费事件；上层复用本类型时（如 [`crate::WorkspaceBuffer`]）应改走 [`Self::apply_pending_events`]——自己 `take_pending_events` 后扇出给 search + document。
    pub fn pump_post_edit(&mut self) -> WorkspaceResult<()> {
        let events = self.buffer.take_pending_events();
        self.apply_pending_events(&events);
        Ok(())
    }

    /// 把已经从 buffer 排空的 [`DeltaEvent`] 喂给 syntax。
    pub fn apply_pending_events(&mut self, events: &[DeltaEvent]) {
        if events.is_empty() {
            return;
        }
        if let Some(state) = self.syntax.as_ref() {
            for event in events {
                state.handle_edit(&self.buffer, event);
            }
        }
    }

    /// 按构造期钉死的 [`Self::language`] 挂 provider。
    /// `LanguageId::PLAIN` / 超阈值 / 注册表里没有该语言时静默走 plain 路径。
    fn attach_syntax(&mut self) {
        debug_assert!(self.syntax.is_none(), "重复 attach_syntax");
        if self.buffer.snapshot().len_bytes().get() > MAX_HIGHLIGHT_BYTES {
            return;
        }
        if self.language.is_plain() {
            return;
        }
        let Some(provider) = self.engine.registry().make_provider(self.language) else {
            return;
        };
        let state = BufferSyntax::attach(
            self.buffer_id,
            self.language,
            provider,
            &self.buffer,
            self.engine.worker().clone(),
        );
        self.syntax = Some(state);
    }
}

impl Drop for SyntaxDocument {
    fn drop(&mut self) {
        if let Some(state) = self.syntax.take() {
            state.detach();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::providers::install_builtin_providers;

    fn engine_with_builtins() -> Rc<SyntaxEngine> {
        let mut engine = SyntaxEngine::new();
        install_builtin_providers(&mut engine);
        Rc::new(engine)
    }

    #[test]
    fn explicit_language_attaches_provider_and_populates_slot() {
        let engine = engine_with_builtins();
        let doc = SyntaxDocument::with_text(engine.clone(), LanguageId::new("rust"), "fn x() {}")
            .unwrap();
        engine.worker().wait_for_idle();
        let tree = doc
            .syntax_tree_slot()
            .expect("rust buffer 必须挂 provider")
            .load()
            .expect("attach 完成后 slot 必须有 tree");
        assert_eq!(tree.version(), doc.buffer().version());
    }

    #[test]
    fn replace_text_keeps_language_and_clears_old_slot_before_re_attach() {
        let engine = engine_with_builtins();
        let mut doc =
            SyntaxDocument::with_text(engine.clone(), LanguageId::new("rust"), "fn x() {}")
                .unwrap();
        engine.worker().wait_for_idle();
        let old_slot = doc.syntax_tree_slot().unwrap().clone();
        assert!(old_slot.load().is_some());

        doc.replace_text("// new text").unwrap();
        // 旧 slot 在 replace_text 内的 detach 路径上已被清空。
        assert!(old_slot.load().is_none());
        engine.worker().wait_for_idle();
        // 新 slot 在新 syntax 内：仍能拿到 tree。
        assert!(doc.syntax_tree_slot().unwrap().load().is_some());
        assert_eq!(doc.language(), LanguageId::new("rust"));
    }

    #[test]
    fn plain_language_yields_no_syntax_state() {
        let engine = engine_with_builtins();
        let doc = SyntaxDocument::with_text(engine, LanguageId::PLAIN, "hello world").unwrap();
        assert!(doc.language().is_plain());
        assert!(doc.syntax_tree_slot().is_none());
    }
}
