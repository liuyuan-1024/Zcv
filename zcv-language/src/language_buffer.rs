use std::path::{Path, PathBuf};

use gpui::{AppContext, Context, Entity, Task};
use zcv_engine::{Buffer, Snapshot, TextSubscription};

use crate::registry::language_name_for_file;
use crate::syntax_map::{SyntaxMap, SyntaxSnapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseStatus {
    Idle,
    Parsing,
}

/// 将文本 Buffer 与语言派生状态绑定在一起。
///
/// 和 Zed 的 language::Buffer 一样，语法树跟随文本而不是某个 Editor。
/// 多个 Editor 可以共享一个 `LanguageBuffer`，后台也只会存在一个解析任务。
pub struct LanguageBuffer {
    buffer: Entity<Buffer>,
    subscription: TextSubscription,
    text_snapshot: Snapshot,
    syntax_map: SyntaxMap,
    file_path: Option<PathBuf>,
    parse_task: Option<Task<()>>,
    parse_again: bool,
    parse_status: ParseStatus,
}

impl LanguageBuffer {
    pub fn new(buffer: Entity<Buffer>, file_path: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let (subscription, snapshot) =
            buffer.update(cx, |buffer, _| (buffer.subscribe(), buffer.snapshot()));
        let mut syntax_map = SyntaxMap::new(&snapshot);
        if let Some(path) = file_path.as_deref() {
            syntax_map.set_language_for_file(path, &snapshot);
        }

        cx.observe(&buffer, |language_buffer, _, cx| {
            language_buffer.sync(cx);
        })
        .detach();

        let mut this = Self {
            buffer,
            subscription,
            text_snapshot: snapshot,
            syntax_map,
            file_path,
            parse_task: None,
            parse_again: false,
            parse_status: ParseStatus::Idle,
        };
        this.start_reparse(cx);
        this
    }

    pub fn buffer(&self) -> Entity<Buffer> {
        self.buffer.clone()
    }

    pub fn file_path(&self) -> Option<&Path> {
        self.file_path.as_deref()
    }

    pub fn language_name(&self) -> Option<&'static str> {
        let path = self.file_path.as_deref()?;
        let first_line = self
            .text_snapshot
            .slice_line(zcv_engine::Line::ZERO)
            .ok()
            .map(|line| line.as_str().trim_end_matches(['\r', '\n']).to_owned());
        language_name_for_file(path, first_line.as_deref())
    }

    pub fn set_file_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.sync(cx);
        let language_changed = self
            .syntax_map
            .set_language_for_file(&path, &self.text_snapshot);
        self.file_path = Some(path);
        if language_changed {
            self.start_reparse(cx);
        }
        cx.notify();
    }

    pub fn syntax_snapshot(&self) -> SyntaxSnapshot {
        self.syntax_map.snapshot()
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let changes = self.subscription.consume();
        if changes.is_empty() {
            return;
        }
        let new_snapshot = self.buffer.read(cx).snapshot();
        self.syntax_map
            .interpolate(&self.text_snapshot, &new_snapshot, &changes);
        self.text_snapshot = new_snapshot;
        if let Some(path) = self.file_path.as_deref() {
            self.syntax_map
                .set_language_for_file(path, &self.text_snapshot);
        }
        self.start_reparse(cx);
        cx.notify();
    }

    fn start_reparse(&mut self, cx: &mut Context<Self>) {
        if !self.syntax_map.snapshot().has_language() {
            self.parse_status = ParseStatus::Idle;
            return;
        }
        if self.parse_task.is_some() {
            self.parse_again = true;
            return;
        }

        self.parse_again = false;
        self.parse_status = ParseStatus::Parsing;
        let text = self.text_snapshot.clone();
        let syntax = self.syntax_map.snapshot();
        let parse_task = cx.background_spawn(async move { syntax.reparse(&text) });
        self.parse_task = Some(cx.spawn(async move |this, cx| {
            let parsed = parse_task.await;
            let _ = this.update(cx, |this, cx| {
                this.parse_task = None;
                let parsed_version = parsed.version();
                let installed = this.syntax_map.did_parse(parsed);
                let parse_again = this.parse_again
                    || parsed_version != this.text_snapshot.version()
                    || !installed;
                this.parse_again = false;
                this.parse_status = ParseStatus::Idle;
                if installed {
                    cx.notify();
                }
                if parse_again {
                    this.start_reparse(cx);
                }
            });
        }));
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;
    use zcv_engine::{BufferConfig, ByteOffset, Edit, TransactionMetadata};

    use super::*;

    #[gpui::test]
    fn parsing_finishes_without_blocking_buffer_edits(cx: &mut TestAppContext) {
        let buffer = cx.new(|_| {
            Buffer::scratch("fn main() {}\n".to_owned(), BufferConfig::default())
                .expect("应创建测试 Buffer")
        });
        let language_buffer =
            cx.new(|cx| LanguageBuffer::new(buffer.clone(), Some(PathBuf::from("main.rs")), cx));

        buffer.update(cx, |buffer, cx| {
            buffer
                .edit(
                    [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                    TransactionMetadata::default(),
                )
                .expect("测试编辑应成功");
            cx.notify();
        });
        cx.run_until_parked();

        let buffer_version = cx.read_entity(&buffer, |buffer, _| buffer.version());
        language_buffer.read_with(cx, |language_buffer, _| {
            let syntax = language_buffer.syntax_snapshot();
            assert_eq!(syntax.version(), buffer_version);
        });
    }

    #[gpui::test]
    fn language_name_and_syntax_follow_first_line_changes(cx: &mut TestAppContext) {
        let buffer = cx.new(|_| {
            Buffer::scratch(String::new(), BufferConfig::default()).expect("应创建测试 Buffer")
        });
        let language_buffer =
            cx.new(|cx| LanguageBuffer::new(buffer.clone(), Some(PathBuf::from("script")), cx));

        cx.read_entity(&language_buffer, |language_buffer, _| {
            assert_eq!(language_buffer.language_name(), None)
        });
        buffer.update(cx, |buffer, cx| {
            buffer
                .edit(
                    [
                        Edit::insert(ByteOffset::ZERO, "#!/usr/bin/env python\nprint('ok')\n")
                            .unwrap(),
                    ],
                    TransactionMetadata::default(),
                )
                .expect("测试编辑应成功");
            cx.notify();
        });
        cx.run_until_parked();

        language_buffer.read_with(cx, |language_buffer, _| {
            assert_eq!(language_buffer.language_name(), Some("Python"));
            assert!(language_buffer.syntax_snapshot().has_language());
        });
    }
}
