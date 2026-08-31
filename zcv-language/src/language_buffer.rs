use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use gpui::{AppContext, Context, Entity, EventEmitter, Task};
use zcv_text::{Buffer, Line, Snapshot, TextChangeBatch, TextSubscription};

use crate::FoldRange;
use crate::Language;
use crate::syntax_map::{SyntaxMap, SyntaxSnapshot, edit_ranges};
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

/// 后台解析 + 折叠计算的完成信号：结果放 Mutex，Condvar 唤醒可能正在等待的主线程。
///
/// ~1ms 同步解析预算：主线程在编辑轮内短等待极快的增量解析，完成后直接安装新鲜语法，显示不必停留在插值树。
type ParseCompletion = (Mutex<Option<ParseOutcome>>, Condvar);

type ParseOutcome = (SyntaxSnapshot, Vec<FoldRange>);

/// 主线程等待后台解析的最长时间。
const SYNC_PARSE_TIMEOUT: Duration = Duration::from_millis(1);

struct ParseTask {
    cancellation: ParseCancellation,
    _task: Task<()>,
    completion: Arc<ParseCompletion>,
}

impl ParseTask {
    /// 短等待后台解析完成（超时或取消返回 None）。
    fn wait_completion(&self, timeout: Duration) -> Option<ParseOutcome> {
        wait_parse_completion(&self.completion, timeout)
    }
}

/// 短等待后台解析完成：结果已就绪立即返回，否则阻塞至超时（~1ms 同步解析预算）。
fn wait_parse_completion(completion: &ParseCompletion, timeout: Duration) -> Option<ParseOutcome> {
    let (lock, cvar) = completion;
    let mut guard = lock.lock().expect("解析完成信号锁不应中毒");
    let deadline = Instant::now() + timeout;
    while guard.is_none() {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let (next_guard, _) = cvar
            .wait_timeout(guard, deadline - now)
            .expect("解析完成信号锁不应中毒");
        guard = next_guard;
    }
    guard.take()
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
    /// 折叠派生数据缓存：解析结果在后台安装时计算一次，所有共享本 Buffer 的 Editor 复用。
    /// 文本编辑（TextChanged）不触碰此缓存，只在新解析安装（Reparsed）后整体替换。
    fold_ranges: Arc<[FoldRange]>,
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
            fold_ranges: Arc::from([]),
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
            self.fold_ranges = Arc::from([]);
            self.start_reparse(cx);
        }
        cx.emit(LanguageBufferEvent::MetadataChanged);
        cx.notify();
    }

    pub fn syntax_snapshot(&self) -> SyntaxSnapshot {
        self.syntax_map.snapshot()
    }

    /// 当前已安装解析对应的折叠范围（Arc 共享，多个 Editor 零拷贝复用，不重复计算）。
    ///
    /// 缓存只在 Reparsed 安装时刷新；
    /// 文本已编辑但新解析未安装的窗口期返回上一版结果，与编辑器只在 Reparsed 后刷新折叠的行为一致。
    pub fn fold_ranges(&self) -> Arc<[FoldRange]> {
        Arc::clone(&self.fold_ranges)
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
        let edits = edit_ranges(&changes);
        self.syntax_map
            .interpolate(&self.text_snapshot, &new_snapshot, &changes);
        self.text_snapshot = new_snapshot;
        if first_line_may_have_changed {
            let next_first_line = first_line(&self.text_snapshot);
            if next_first_line != self.language_detection_first_line {
                self.language_detection_first_line = next_first_line;
                if let Some(path) = self.file_path.as_deref()
                    && self.syntax_map.set_language_for_file(
                        path,
                        Some(&self.language_detection_first_line),
                        &self.text_snapshot,
                    )
                {
                    self.fold_ranges = Arc::from([]);
                }
            }
        }
        self.start_reparse_with_edits(edits, cx);
        cx.emit(LanguageBufferEvent::TextChanged);
        // 极快增量解析赶上当前按键：编辑轮内直接安装新鲜语法（见 install_sync_parse_result）。
        self.install_sync_parse_result(cx);
        cx.notify();
    }

    fn start_reparse(&mut self, cx: &mut Context<Self>) {
        self.start_reparse_with_edits(None, cx);
    }

    fn start_reparse_with_edits(
        &mut self,
        edits: Option<Vec<std::ops::Range<usize>>>,
        cx: &mut Context<Self>,
    ) {
        // `ParseTask::drop` 会先通知 Tree-sitter 中止旧工作，再取消等待结果的前台任务。
        self.parse_task = None;
        if self
            .syntax_map
            .language()
            .and_then(Language::grammar)
            .is_none()
        {
            return;
        }

        let text = self.text_snapshot.clone();
        let syntax = self.syntax_map.snapshot();
        let cancellation = ParseCancellation::default();
        let parse_cancellation = cancellation.clone();
        let completion: Arc<ParseCompletion> = Arc::default();
        let task_completion = Arc::clone(&completion);
        // 折叠派生数据与解析同批在后台计算：主线程在 Reparsed 安装时只做一次 Arc 级替换，共享同一 LanguageBuffer 的多个 Editor 不再各自跑一遍全量折叠查询。
        // 完成后置入完成信号：正在主线程短等待的 sync 可以直接同步安装新鲜语法。
        let parse_task = cx.background_spawn(async move {
            let outcome = (|| {
                let parsed = syntax.reparse(&text, edits.as_deref(), &parse_cancellation)?;
                let folds = parsed.fold_ranges(0..text.len_bytes().get(), &text);
                Some((parsed, folds))
            })();
            let (lock, cvar) = &*task_completion;
            *lock.lock().expect("解析完成信号锁不应中毒") = outcome.clone();
            cvar.notify_one();
            outcome
        });
        let task = cx.spawn(async move |this, cx| {
            let Some((parsed, folds)) = parse_task.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                // 结果已被 sync 同步安装（parse_task 已替换为 None）时不再重复安装。
                this.parse_task = None;
                let installed = this.syntax_map.did_parse(parsed);
                if installed {
                    this.fold_ranges = Arc::from(folds);
                    cx.emit(LanguageBufferEvent::Reparsed);
                    cx.notify();
                }
            });
        });
        self.parse_task = Some(ParseTask {
            cancellation,
            _task: task,
            completion,
        });
    }

    /// 主线程短等待后台解析：极快增量解析（通常远小于 1ms）赶上当前按键时，在编辑轮内直接安装新鲜语法与折叠派生数据，显示不停留在插值树（~1ms 同步解析预算；超时则保持原异步路径，稍后经 Reparsed 安装）。
    fn install_sync_parse_result(&mut self, cx: &mut Context<Self>) {
        let Some(parse_task) = self.parse_task.as_ref() else {
            return;
        };
        let Some((parsed, folds)) = parse_task.wait_completion(SYNC_PARSE_TIMEOUT) else {
            return;
        };
        if self.syntax_map.did_parse(parsed) {
            self.fold_ranges = Arc::from(folds);
            // 结果已同步安装：丢弃异步安装路径（ParseTask::drop 取消后台任务）。
            self.parse_task = None;
            cx.emit(LanguageBufferEvent::Reparsed);
        }
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
    use zcv_text::{BufferConfig, BufferVersion, ByteOffset, Edit, TransactionMetadata};

    use super::*;

    #[test]
    fn sync_parse_wait_returns_completed_result_within_timeout() {
        // 后台解析（真实线程）完成前主线程阻塞等待，完成后立即返回结果。
        let completion: Arc<ParseCompletion> = Arc::default();
        let worker = Arc::clone(&completion);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(5));
            let (lock, cvar) = &*worker;
            *lock.lock().expect("解析完成信号锁不应中毒") =
                Some((SyntaxSnapshot::empty(BufferVersion::INITIAL), Vec::new()));
            cvar.notify_one();
        });
        let outcome = wait_parse_completion(&completion, Duration::from_millis(100));
        assert!(outcome.is_some(), "已完成的解析应在超时前被主线程取到");
    }

    #[test]
    fn sync_parse_wait_times_out_when_parse_is_slow() {
        // 超过预算的解析：等待超时返回 None，留给后台任务稍后经 Reparsed 安装。
        let completion: Arc<ParseCompletion> = Arc::default();
        let start = Instant::now();
        let outcome = wait_parse_completion(&completion, Duration::from_millis(10));
        assert!(outcome.is_none(), "慢解析等待应超时");
        assert!(
            start.elapsed() >= Duration::from_millis(8),
            "等待应消耗接近完整的预算"
        );
    }

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
            assert!(
                language_buffer.parse_task.is_none(),
                "纯文本不应启动无意义的后台解析任务"
            );
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
    /// 快速连续编辑：测试环境的后台任务由确定性调度驱动（不与主线程并发），
    /// sync 的 ~1ms 等待总是超时，中间解析被取消，只安装最新一次（1 次 Reparsed）。
    /// 生产环境（真实线程池）中每次编辑的极快解析会在编辑轮内同步安装，事件数可能更多，但任何时刻安装的语法都与当次文本版本一致。
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
