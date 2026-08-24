//! Editor 与具体文本 Buffer 之间的组合文档边界。
//!
//! 当前 Zcv 的可见行为仍然是一份 Item 对应一份文档，因此这里只开放 singleton 构造；
//! Editor 的唯一文档实体与 DisplayMap 的快照输入都来自本层。

use std::path::{Path, PathBuf};

use gpui::{App, Context, Entity};
use zcv_language::{AutoClosePair, LanguageBuffer, SyntaxSnapshot};
use zcv_text::{Buffer, BufferVersion, ByteOffset, Snapshot, TextChangeBatch, TextSubscription};

/// 一帧组合文档的不可变快照。
#[derive(Clone, Debug)]
pub struct MultiBufferSnapshot {
    text: Snapshot,
    syntax: SyntaxSnapshot,
}

/// Editor 对组合文档变化的独立订阅。
///
/// singleton 阶段内部复用文本引擎订阅；消费者不再依赖底层 Buffer 的订阅类型，后续扩展 ordered excerpts 时可在本层组合多个来源的变化。
pub struct MultiBufferSubscription {
    text: TextSubscription,
}

impl MultiBufferSubscription {
    pub fn consume(&mut self) -> TextChangeBatch {
        self.text.consume()
    }
}

impl MultiBufferSnapshot {
    pub fn singleton(text: Snapshot, syntax: SyntaxSnapshot) -> Self {
        Self { text, syntax }
    }

    pub fn text(&self) -> &Snapshot {
        &self.text
    }

    pub fn syntax(&self) -> &SyntaxSnapshot {
        &self.syntax
    }

    /// 返回当前组合文档的完整 UTF-8 内容，供预览等只读消费者使用。
    pub fn text_bytes(&self) -> Vec<u8> {
        self.text
            .slice_byte_range(ByteOffset::ZERO, self.text.len_bytes())
            .expect("完整快照范围必须有效")
            .as_str()
            .as_bytes()
            .to_vec()
    }

    pub fn version(&self) -> BufferVersion {
        self.text.version()
    }
}

impl From<Snapshot> for MultiBufferSnapshot {
    fn from(text: Snapshot) -> Self {
        let syntax = SyntaxSnapshot::empty(text.version());
        Self { text, syntax }
    }
}

/// Editor 持有的组合文档模型。
pub struct MultiBuffer {
    singleton: Entity<LanguageBuffer>,
}

impl MultiBuffer {
    pub fn singleton(singleton: Entity<LanguageBuffer>, cx: &mut Context<Self>) -> Self {
        let text_buffer = singleton.read(cx).buffer();
        cx.observe(&singleton, |_, _, cx| cx.notify()).detach();
        cx.observe(&text_buffer, |_, _, cx| cx.notify()).detach();
        Self { singleton }
    }

    pub fn snapshot(&self, cx: &App) -> MultiBufferSnapshot {
        let language_buffer = self.singleton.read(cx);
        MultiBufferSnapshot::singleton(
            language_buffer.buffer().read(cx).snapshot(),
            language_buffer.syntax_snapshot(),
        )
    }

    /// singleton 对应的底层文本；组合文档返回 None。
    pub fn as_singleton(&self, cx: &App) -> Option<Entity<Buffer>> {
        Some(self.singleton.read(cx).buffer())
    }

    pub fn is_dirty(&self, cx: &App) -> bool {
        self.singleton.read(cx).buffer().read(cx).is_dirty()
    }

    pub fn subscribe_and_snapshot(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (MultiBufferSubscription, MultiBufferSnapshot) {
        let language_buffer = self.singleton.read(cx);
        let syntax = language_buffer.syntax_snapshot();
        let text_buffer = language_buffer.buffer();
        let (subscription, text) =
            text_buffer.update(cx, |buffer, _| (buffer.subscribe(), buffer.snapshot()));
        (
            MultiBufferSubscription { text: subscription },
            MultiBufferSnapshot::singleton(text, syntax),
        )
    }

    pub fn file_path(&self, cx: &App) -> Option<PathBuf> {
        self.singleton.read(cx).file_path().map(Path::to_path_buf)
    }

    pub fn set_file_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.singleton
            .update(cx, |buffer, cx| buffer.set_file_path(path, cx));
    }

    pub fn language_name(&self, cx: &App) -> Option<&'static str> {
        self.singleton.read(cx).language_name()
    }

    pub fn auto_close_pairs(&self, cx: &App) -> Option<&'static [AutoClosePair]> {
        Some(self.singleton.read(cx).language()?.auto_close_pairs())
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext as _, TestAppContext};
    use zcv_text::{BufferConfig, ByteOffset, Edit, TransactionMetadata};

    use super::*;

    #[gpui::test]
    fn singleton_snapshot_tracks_text_syntax_and_path(cx: &mut TestAppContext) {
        let text_buffer = cx.new(|_| {
            Buffer::scratch("fn main() {}\n".to_owned(), BufferConfig::default())
                .expect("应创建测试 Buffer")
        });
        let language_buffer = cx.new(|cx| {
            LanguageBuffer::new(text_buffer.clone(), Some(PathBuf::from("src/main.rs")), cx)
        });
        let multi_buffer = cx.new(|cx| MultiBuffer::singleton(language_buffer, cx));

        let first = multi_buffer.read_with(cx, |buffer, cx| buffer.snapshot(cx));
        assert_eq!(first.text().version(), first.syntax().version());
        assert_eq!(
            multi_buffer.read_with(cx, |buffer, cx| buffer.file_path(cx)),
            Some(PathBuf::from("src/main.rs"))
        );

        text_buffer.update(cx, |buffer, cx| {
            buffer
                .edit(
                    [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                    TransactionMetadata::default(),
                )
                .expect("测试编辑应成功");
            cx.notify();
        });
        cx.run_until_parked();

        let updated = multi_buffer.read_with(cx, |buffer, cx| buffer.snapshot(cx));
        assert_eq!(updated.text().version(), updated.syntax().version());
        assert_eq!(
            multi_buffer.read_with(cx, |buffer, cx| buffer.as_singleton(cx).unwrap()),
            text_buffer
        );
    }
}
