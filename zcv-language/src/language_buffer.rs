use std::path::{Path, PathBuf};

use gpui::{AppContext, Context, Entity, EventEmitter, Task};
use zcv_text::{Buffer, Line, Snapshot, TextChangeBatch, TextSubscription};

use crate::Language;
use crate::syntax_map::{SyntaxMap, SyntaxSnapshot};
use crate::tree_sitter_utils::ParseCancellation;

/// 语言 Buffer 的显式更新语义；
/// 文本插值、后台解析和元数据变化具有不同消费成本。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanguageBufferEvent {
    TextChanged,
    Reparsed,
    MetadataChanged,
}

impl EventEmitter<LanguageBufferEvent> for LanguageBuffer {}

struct ParseTask {
    cancellation: ParseCancellation,
    _task: Task<()>,
}

impl Drop for ParseTask {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

/// 将文本 Buffer 与语言派生状态绑定在一起。
///
/// 语法树跟随文本而不是某个 Editor。
/// 多个 Editor 可以共享一个 `LanguageBuffer`，后台也只会存在一个解析任务。
pub struct LanguageBuffer {
    buffer: Entity<Buffer>,
    subscription: TextSubscription,
    text_snapshot: Snapshot,
    syntax_map: SyntaxMap,
    file_path: Option<PathBuf>,
    language_detection_first_line: String,
    parse_task: Option<ParseTask>,
}

impl LanguageBuffer {
    pub fn new(buffer: Entity<Buffer>, file_path: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let (subscription, snapshot) =
            buffer.update(cx, |buffer, _| (buffer.subscribe(), buffer.snapshot()));
        let first_line = first_line(&snapshot);
        let mut syntax_map = SyntaxMap::new(&snapshot);
        if let Some(path) = file_path.as_deref() {
            syntax_map.set_language_for_file(path, Some(&first_line), &snapshot);
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
            language_detection_first_line: first_line,
            parse_task: None,
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

    /// 当前语言引用（编辑器输入行为等消费方取语言配置用，不克隆语法快照）。
    pub fn language(&self) -> Option<&Language> {
        self.syntax_map.language()
    }

    pub fn language_name(&self) -> Option<&'static str> {
        self.file_path.as_ref()?;
        self.language().map(Language::name)
    }

    pub fn set_file_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.sync(cx);
        let language_changed = self.syntax_map.set_language_for_file(
            &path,
            Some(&self.language_detection_first_line),
            &self.text_snapshot,
        );
        self.file_path = Some(path);
        if language_changed {
            self.start_reparse(cx);
        }
        cx.emit(LanguageBufferEvent::MetadataChanged);
        cx.notify();
    }

    pub fn syntax_snapshot(&self) -> SyntaxSnapshot {
        self.syntax_map.snapshot()
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let changes = self.subscription.consume();
        if changes.is_empty() {
            cx.emit(LanguageBufferEvent::MetadataChanged);
            cx.notify();
            return;
        }
        let new_snapshot = self.buffer.read(cx).snapshot();
        let first_line_may_have_changed =
            changes_touch_first_line(&self.text_snapshot, &new_snapshot, &changes);
        self.syntax_map
            .interpolate(&self.text_snapshot, &new_snapshot, &changes);
        self.text_snapshot = new_snapshot;
        if first_line_may_have_changed {
            let next_first_line = first_line(&self.text_snapshot);
            if next_first_line != self.language_detection_first_line {
                self.language_detection_first_line = next_first_line;
                if let Some(path) = self.file_path.as_deref() {
                    self.syntax_map.set_language_for_file(
                        path,
                        Some(&self.language_detection_first_line),
                        &self.text_snapshot,
                    );
                }
            }
        }
        self.start_reparse(cx);
        cx.emit(LanguageBufferEvent::TextChanged);
        cx.notify();
    }

    fn start_reparse(&mut self, cx: &mut Context<Self>) {
        // `ParseTask::drop` 会先通知 Tree-sitter 中止旧工作，再取消等待结果的前台任务。
        self.parse_task = None;
        if self.syntax_map.language().is_none() {
            return;
        }

        let text = self.text_snapshot.clone();
        let syntax = self.syntax_map.snapshot();
        let cancellation = ParseCancellation::default();
        let parse_cancellation = cancellation.clone();
        let parse_task =
            cx.background_spawn(async move { syntax.reparse(&text, &parse_cancellation) });
        let task = cx.spawn(async move |this, cx| {
            let Some(parsed) = parse_task.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.parse_task = None;
                let installed = this.syntax_map.did_parse(parsed);
                if installed {
                    cx.emit(LanguageBufferEvent::Reparsed);
                    cx.notify();
                }
            });
        });
        self.parse_task = Some(ParseTask {
            cancellation,
            _task: task,
        });
    }
}

fn first_line(snapshot: &Snapshot) -> String {
    snapshot
        .slice_line(Line::ZERO)
        .expect("文本快照始终至少包含第 0 行")
        .as_str()
        .trim_end_matches(['\r', '\n'])
        .to_owned()
}

fn changes_touch_first_line(
    old_snapshot: &Snapshot,
    new_snapshot: &Snapshot,
    changes: &TextChangeBatch,
) -> bool {
    if changes.requires_reset() {
        return true;
    }
    changes.patch().edits().iter().any(|edit| {
        offset_touches_first_line(old_snapshot, edit.old_range().start().get())
            || offset_touches_first_line(new_snapshot, edit.new_range().start().get())
    })
}

fn offset_touches_first_line(snapshot: &Snapshot, offset: usize) -> bool {
    if snapshot.line_count() == 1 {
        offset <= snapshot.len_bytes().get()
    } else {
        offset
            < snapshot
                .line_start_byte(Line::new(1))
                .expect("多行快照必须存在第 1 行")
                .get()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::TestAppContext;
    use zcv_text::{BufferConfig, ByteOffset, Edit, TransactionMetadata};

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
            // 未识别文件以”纯文本“兜底，且无语法树。
            assert_eq!(language_buffer.language_name(), Some("纯文本"));
            let language = language_buffer.language().expect("兜底语言应存在");
            assert_eq!(language.name(), "纯文本");
            assert!(language.grammar().is_none(), "纯文本兜底不应有语法树");
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

    #[gpui::test]
    fn distinguishes_text_parse_and_metadata_events(cx: &mut TestAppContext) {
        let buffer = cx.new(|_| {
            Buffer::scratch("fn main() {}\n".to_owned(), BufferConfig::default())
                .expect("应创建测试 Buffer")
        });
        let language_buffer =
            cx.new(|cx| LanguageBuffer::new(buffer.clone(), Some(PathBuf::from("main.rs")), cx));
        cx.run_until_parked();

        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = Rc::clone(&events);
        let _subscription = cx.update(|cx| {
            cx.subscribe(&language_buffer, move |_, event, _| {
                observed.borrow_mut().push(*event);
            })
        });

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
        assert_eq!(
            events.borrow().as_slice(),
            [
                LanguageBufferEvent::TextChanged,
                LanguageBufferEvent::Reparsed
            ]
        );

        events.borrow_mut().clear();
        buffer.update(cx, |buffer, cx| {
            buffer.mark_saved();
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(
            events.borrow().as_slice(),
            [LanguageBufferEvent::MetadataChanged]
        );
    }

    #[gpui::test]
    fn rapid_edits_install_only_the_latest_parse(cx: &mut TestAppContext) {
        let buffer = cx.new(|_| {
            Buffer::scratch("fn main() {}\n".to_owned(), BufferConfig::default())
                .expect("应创建测试 Buffer")
        });
        let language_buffer =
            cx.new(|cx| LanguageBuffer::new(buffer.clone(), Some(PathBuf::from("main.rs")), cx));
        cx.run_until_parked();

        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = Rc::clone(&events);
        let _subscription = cx.update(|cx| {
            cx.subscribe(&language_buffer, move |_, event, _| {
                observed.borrow_mut().push(*event);
            })
        });

        for text in ["a", "b", "c"] {
            buffer.update(cx, |buffer, cx| {
                buffer
                    .edit(
                        [Edit::insert(buffer.len_bytes(), text).unwrap()],
                        TransactionMetadata::default(),
                    )
                    .expect("测试编辑应成功");
                cx.notify();
            });
        }
        let latest_version = cx.read_entity(&buffer, |buffer, _| buffer.version());
        cx.run_until_parked();

        language_buffer.read_with(cx, |language_buffer, _| {
            assert_eq!(language_buffer.syntax_snapshot().version(), latest_version);
        });
        assert_eq!(
            events
                .borrow()
                .iter()
                .filter(|event| **event == LanguageBufferEvent::Reparsed)
                .count(),
            1
        );
    }
}
