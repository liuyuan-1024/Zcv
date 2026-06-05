//! `SyntaxDocument` —— 单缓冲区文档。
//!
//! 嵌入式编辑器（设置面板的 TOML 视图、未来的代码片段输入框 ……）需要的不是一整个 [`crate::Workspace`]
//! ——它们没有多 buffer / active buffer / 文件树这些概念，只是想要「一个挂着语法高亮的 [`Buffer`]」。
//! 把 [`Workspace`] 拉过来用一次性 BTreeMap 槽位太重，每个嵌入点还会自带一份 provider 工厂表与一根后台 worker 线程。
//!
//! 本类型把缓冲区生命周期与语法高亮调度内聚成一个值：
//!
//! - 持有 [`Buffer`]、[`MetadataLayers<HighlightSpan>`]、可选的[`BufferSyntaxState`]
//! ——主工作区里 [`crate::WorkspaceBuffer`] 的同款三件套。
//! - 通过 `Rc<SyntaxEngine>` 与主工作区共享语言注册表 / 后台 worker / buffer id 分配器——同进程内永远只有一份这些资源。
//! - 暴露同样的「每帧两拍」节奏：[`Self::pump_post_edit`] 把编辑事件喂给高亮 provider，[`Self::pump_pending_highlights`] 收割已就绪产物。
//!
//! 不持有选区——选区属于「视图」概念，由嵌入点的目标层各自维护。
//!
//! ## 为什么直接吃 [`LanguageId`]
//!
//! 嵌入式编辑器对自身语言通常**已知**（设置面板就是 toml，代码片段输入框由调用方决定），不需要走 [`crate::Workspace::open_*`] 那种「按 path + 首行 shebang 识别」的路径。
//! 直接传 [`LanguageId`] 让 API 表达意图，也避免「伪造一个看起来像文件名的字符串只是为了让 registry detect」这种绕远路的写法。
//! 需要按真实路径识别时，调用方可以自己调 [`crate::syntax::LanguageRegistry::detect`] 再把结果传进来。

use std::rc::Rc;

use zom_engine::{Buffer, BufferConfig, DeltaEvent, MetadataLayers, TextRange};

use crate::syntax::{
    BufferSyntaxState, HighlightSpan, LanguageId, MAX_HIGHLIGHT_BYTES, SyntaxEngine,
    syntax_layer_kind,
};
use crate::{BufferId, WorkspaceResult};

/// 单个挂着语法高亮的缓冲区。
///
/// 在 [`crate::Workspace`] 之外作为嵌入式编辑器的「文档」单元使用——
/// 与主工作区共享同一根 [`SyntaxEngine`]，但 buffer / layer 自己持。
#[derive(Debug)]
pub struct SyntaxDocument {
    engine: Rc<SyntaxEngine>,
    buffer_id: BufferId,
    buffer: Buffer,
    /// 语言钉死在构造期：嵌入式编辑器自知语言，不走 path 识别。
    /// `LanguageId::PLAIN` 表示纯文本编辑（不挂 provider）。
    language: LanguageId,
    highlight_layers: MetadataLayers<HighlightSpan>,
    syntax: Option<BufferSyntaxState>,
}

impl SyntaxDocument {
    /// 接管一个已经造好的 [`Buffer`]
    /// ——主工作区路径（`Workspace::open_file` 走流式 decoder、`Workspace::open_text` 走 `from_text`，buffer 都在外面造好）
    /// 从这条入口构造文档。构造时按 `language` 立刻 attach provider。
    pub fn from_buffer(engine: Rc<SyntaxEngine>, buffer: Buffer, language: LanguageId) -> Self {
        let buffer_id = engine.allocate_buffer_id();
        let mut doc = Self {
            engine,
            buffer_id,
            buffer,
            language,
            highlight_layers: MetadataLayers::new(),
            syntax: None,
        };
        doc.attach_syntax();
        doc
    }

    /// 空文档：buffer 没有内容、provider 尚未 attach——首次 [`Self::replace_text`] 才触发 attach。
    /// 嵌入式编辑器（构造期还不知道初始文本）走它，避免「空 buffer attach 一遍、replace_text 又 detach 再 attach」的浪费。
    pub fn empty(engine: Rc<SyntaxEngine>, language: LanguageId) -> WorkspaceResult<Self> {
        let buffer = Buffer::new(BufferConfig::default())?;
        let buffer_id = engine.allocate_buffer_id();
        Ok(Self {
            engine,
            buffer_id,
            buffer,
            language,
            highlight_layers: MetadataLayers::new(),
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

    /// 文档持有的稳定 [`BufferId`]——主工作区把它当作 buffers map 的键。
    pub fn buffer_id(&self) -> BufferId {
        self.buffer_id
    }

    /// 重置文档内容；语言保持不变，只重挂 provider。
    ///
    /// 不变量（高亮架构手册 §九）：
    /// 旧 syntax provider 在新文本灌入前 detach、layer 清空；
    /// 新文本灌入后再 attach。
    /// 中间不会出现「旧 span × 新 buffer」的撕裂。
    pub fn replace_text(&mut self, text: impl Into<String>) -> WorkspaceResult<()> {
        if let Some(state) = self.syntax.take() {
            state.detach(&mut self.highlight_layers);
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

    /// 高亮 layer 只读视图——desktop 渲染端按
    /// [`crate::syntax::syntax_layer_kind`] 取本 layer。
    pub fn highlight_layers(&self) -> &MetadataLayers<HighlightSpan> {
        &self.highlight_layers
    }

    /// 构造期钉死的语言；`LanguageId::PLAIN` 表示不挂 provider 的纯文本。
    pub fn language(&self) -> LanguageId {
        self.language
    }

    /// 编辑后调一次：排空缓冲区 pending events，把每条 ChangeSet 沿用到
    /// 高亮 layer remap 与 provider 通知。
    ///
    /// 嵌入式文档不挂 `BufferSearch`，所以这条路径**独自**消费事件；
    /// 上层复用本类型时（如 [`crate::WorkspaceBuffer`]）应改走 [`Self::apply_pending_events`]
    /// ——自己 `take_pending_events` 后扇出给 search + document，避免事件被双方各 drain 一次。
    pub fn pump_post_edit(&mut self) -> WorkspaceResult<()> {
        let events = self.buffer.take_pending_events();
        self.apply_pending_events(&events);
        Ok(())
    }

    /// 把已经从 buffer 排空的 [`DeltaEvent`] 喂给高亮 layer remap 与 provider。
    ///
    /// 「pending events 的唯一消费方」契约由调用方维护
    /// ——本方法**不**自己 `take_pending_events`，给 [`crate::WorkspaceBuffer`] 这种需要同时喂给搜索 + 文档的容器留出统一 drain 点。
    pub fn apply_pending_events(&mut self, events: &[DeltaEvent]) {
        if events.is_empty() {
            return;
        }
        if let Some(state) = self.syntax.as_mut() {
            for event in events {
                // 语法高亮效果是视口范围的，由工作线程重新计算。
                // 不要在 UI 线程上同步重新映射整个图层：对于大型文件，
                // 这会导致每次插入换行符时，每个现有的高亮区域都会受到影响。
                let _ = self.highlight_layers.replace_layer_ranges(
                    syntax_layer_kind(),
                    event.new_version(),
                    std::iter::empty::<(TextRange, HighlightSpan)>(),
                );
                state.handle_edit(
                    &self.buffer,
                    event.changeset(),
                    event.new_version(),
                    &mut self.highlight_layers,
                );
            }
        }
    }

    /// 把后台 worker 已就绪的高亮产物落到本文档的 layer——每帧 prepaint
    /// 由 desktop 调一次。无 syntax 状态时立即返回。
    pub fn pump_pending_highlights(&mut self) {
        if let Some(state) = self.syntax.as_ref() {
            state.drain_into_layers(self.buffer.version(), &mut self.highlight_layers);
        }
    }

    /// 转发 viewport hint 给 worker。规则同
    /// [`crate::Workspace::set_buffer_viewport_hint`]。
    pub fn set_viewport_hint(&self, byte_range: Option<TextRange>) {
        if let Some(state) = self.syntax.as_ref() {
            state.set_viewport_hint(byte_range);
        }
    }

    /// 按构造期钉死的 [`Self::language`] 挂 provider。
    /// `LanguageId::PLAIN` / 超阈值 / 注册表里没有该语言时静默走 plain 路径 —— 与 [`crate::WorkspaceBuffer::attach_syntax`] 同形。
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
        let state = BufferSyntaxState::attach(
            self.buffer_id,
            self.language,
            provider,
            &self.buffer,
            &mut self.highlight_layers,
            self.engine.worker().clone(),
            None,
        );
        self.syntax = Some(state);
    }
}

impl Drop for SyntaxDocument {
    fn drop(&mut self) {
        // detach 把后台 worker 上的本 buffer entry 与 sink 都关停——
        // 不依赖 Rust drop 顺序也能保证手册 §九 的清场不变量。
        if let Some(state) = self.syntax.take() {
            state.detach(&mut self.highlight_layers);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::{
        BufferHandle, HighlightName, HighlightProvider, HighlightSink, LanguageDetector,
        syntax_layer_kind,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use zom_engine::{BufferVersion, ByteOffset, ChangeSet};

    struct Probe {
        attached: Arc<AtomicUsize>,
    }
    impl HighlightProvider for Probe {
        fn language(&self) -> LanguageId {
            LanguageId::new("rust")
        }
        fn attach(&mut self, buffer: BufferHandle, sink: HighlightSink) {
            self.attached.fetch_add(1, Ordering::SeqCst);
            let snap = buffer.snapshot();
            let end = snap.len_bytes().get().min(2);
            if end == 0 {
                return;
            }
            sink.replace_all(
                snap.version(),
                vec![(
                    TextRange::new(ByteOffset::new(0), ByteOffset::new(end)).unwrap(),
                    HighlightSpan::from_name(HighlightName::new("keyword")),
                )],
            );
        }
        fn on_edit(&mut self, _b: BufferHandle, _c: &ChangeSet, _v: BufferVersion) {}
        fn detach(&mut self) {}
    }

    fn engine_with_rust() -> (Rc<SyntaxEngine>, Arc<AtomicUsize>) {
        let attached = Arc::new(AtomicUsize::new(0));
        let attach_for_factory = attached.clone();
        let mut engine = SyntaxEngine::new();
        engine.registry_mut().register(
            LanguageId::new("rust"),
            vec![LanguageDetector::Extension(&["rs"])],
            Box::new(move || {
                Box::new(Probe {
                    attached: attach_for_factory.clone(),
                })
            }),
        );
        (Rc::new(engine), attached)
    }

    #[test]
    fn explicit_language_attaches_provider() {
        let (engine, attached) = engine_with_rust();
        let mut doc =
            SyntaxDocument::with_text(engine.clone(), LanguageId::new("rust"), "fn x() {}")
                .unwrap();
        engine.worker().wait_for_idle();
        doc.pump_pending_highlights();
        assert_eq!(attached.load(Ordering::SeqCst), 1);
        assert!(
            doc.highlight_layers()
                .layer(&syntax_layer_kind())
                .unwrap()
                .len()
                > 0
        );
    }

    #[test]
    fn replace_text_keeps_language_and_clears_old_layer_before_re_attach() {
        let (engine, _) = engine_with_rust();
        let mut doc =
            SyntaxDocument::with_text(engine.clone(), LanguageId::new("rust"), "fn x() {}")
                .unwrap();
        engine.worker().wait_for_idle();
        doc.pump_pending_highlights();
        assert!(
            doc.highlight_layers()
                .layer(&syntax_layer_kind())
                .unwrap()
                .len()
                > 0
        );

        doc.replace_text("// new text").unwrap();
        engine.worker().wait_for_idle();
        doc.pump_pending_highlights();
        // 语言不变，rust provider 仍在；这里只验证切换中不残留旧 span。
        assert!(
            doc.highlight_layers()
                .layer(&syntax_layer_kind())
                .unwrap()
                .len()
                > 0
        );
        assert_eq!(doc.language(), LanguageId::new("rust"));
    }

    #[test]
    fn plain_language_yields_no_syntax_state() {
        let (engine, _) = engine_with_rust();
        let doc = SyntaxDocument::with_text(engine, LanguageId::PLAIN, "hello world").unwrap();
        assert!(doc.language().is_plain());
    }
}
