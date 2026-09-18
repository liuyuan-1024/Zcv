use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, Entity, EventEmitter, Task};
use zcv_settings::SettingsStore;
use zcv_text::{Buffer, Line, Snapshot, TextSubscription};

use crate::Language;
use crate::highlight_cache::HighlightCache;
use crate::language_settings::LanguageSettings;
use crate::registry::LanguageRegistry;
use crate::syntax_map::{SyntaxMap, SyntaxSnapshot};
use crate::tree_sitter_utils::ParseCancellation;

/// 语言 Buffer 的显式更新语义；
/// 文本插值、后台解析和元数据变化具有不同消费成本。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanguageBufferEvent {
    /// 文本订阅已有新变化；事件只唤醒消费者，不携带可延迟重放的版本化增量。
    TextChanged,
    Reparsed,
    MetadataChanged,
}

impl EventEmitter<LanguageBufferEvent> for LanguageBuffer {}

/// 后台解析 + 折叠计算的完成信号：结果放 Mutex，Condvar 唤醒可能正在等待的主线程。
///
/// ~1ms 同步解析预算：主线程在编辑轮内短等待极快的增量解析，完成后直接安装新鲜语法，显示不必停留在插值树。
type ParseCompletion = (Mutex<Option<ParseOutcome>>, Condvar);

type ParseOutcome = SyntaxSnapshot;

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

/// 语言 Buffer 的一致性快照：文本与语法保证同一版本，并携带语言设置与高亮缓存句柄。
///
/// 消费方应一次取用本快照，而不是分别读取文本与语法；语言层不再提供需要调用方显式推进的手动同步协议。
#[derive(Clone)]
pub struct LanguageBufferSnapshot {
    pub text: Snapshot,
    pub syntax: SyntaxSnapshot,
    pub language: Option<Arc<Language>>,
    pub settings: Arc<LanguageSettings>,
    pub file_path: Option<PathBuf>,
    pub highlight_cache: Arc<HighlightCache>,
}

/// 受同一把锁保护的派生语言状态。
struct LanguageState {
    text_snapshot: Snapshot,
    syntax_map: SyntaxMap,
    detection_first_line: String,
    file_path: Option<PathBuf>,
    settings: Arc<LanguageSettings>,
    highlight_cache: Arc<HighlightCache>,
}

/// 将文本 Buffer 与语言派生状态绑定在一起。
///
/// 语法树跟随文本而不是某个 Editor。
/// 多个 Editor 可以共享一个 `LanguageBuffer`，后台也只会存在一个解析任务。
pub struct LanguageBuffer {
    buffer: Entity<Buffer>,
    subscription: TextSubscription,
    state: Mutex<LanguageState>,
    parse_task: Option<ParseTask>,
}

impl LanguageBuffer {
    pub fn new(
        buffer: Entity<Buffer>,
        file_path: Option<PathBuf>,
        registry: Arc<LanguageRegistry>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (subscription, snapshot) =
            buffer.update(cx, |buffer, _| (buffer.subscribe(), buffer.snapshot()));
        let first_line = first_line(&snapshot);
        let mut syntax_map = SyntaxMap::new(Arc::clone(&registry), &snapshot);
        if let Some(path) = file_path.as_deref() {
            syntax_map.set_language_for_file(path, Some(&first_line), &snapshot);
        }
        let settings = resolve_settings(&syntax_map, cx);

        cx.observe(&buffer, |language_buffer, _, cx| {
            language_buffer.sync(cx);
        })
        .detach();
        // 设置变化时刷新按语言解析的结果；测试环境未注册 SettingsStore 时不建立订阅。
        if cx.try_global::<SettingsStore>().is_some() {
            cx.observe_global::<SettingsStore>(|language_buffer, cx| {
                language_buffer.refresh_settings(cx);
            })
            .detach();
        }

        let mut this = Self {
            buffer,
            subscription,
            state: Mutex::new(LanguageState {
                text_snapshot: snapshot,
                syntax_map,
                detection_first_line: first_line,
                file_path,
                settings,
                highlight_cache: Arc::new(HighlightCache::new()),
            }),
            parse_task: None,
        };
        this.start_reparse(cx);
        this
    }

    pub fn buffer(&self) -> Entity<Buffer> {
        self.buffer.clone()
    }

    /// 本 Buffer 使用的语言注册表；需要创建关联语言 Buffer 的消费方复用同一份注册表。
    pub fn language_registry(&self) -> Arc<LanguageRegistry> {
        self.state
            .lock()
            .expect("语言 Buffer 状态锁不应中毒")
            .syntax_map
            .registry()
    }

    /// 一致性快照：返回前把语法状态推进到文本版本，文本与语法属于同一 `BufferVersion`。
    pub fn snapshot(&self, cx: &App) -> LanguageBufferSnapshot {
        let text = self.buffer.read(cx).snapshot();
        let mut state = self.state.lock().expect("语言 Buffer 状态锁不应中毒");
        advance_state(&mut state, &text, cx);
        LanguageBufferSnapshot {
            text,
            syntax: state.syntax_map.snapshot(),
            language: state.syntax_map.language_arc(),
            settings: Arc::clone(&state.settings),
            file_path: state.file_path.clone(),
            highlight_cache: Arc::clone(&state.highlight_cache),
        }
    }

    /// 文本专用只读快照（不需要语法状态的消费方使用）。
    pub fn text_snapshot(&self, cx: &App) -> Snapshot {
        self.buffer.read(cx).snapshot()
    }

    pub fn file_path(&self) -> Option<PathBuf> {
        self.state
            .lock()
            .expect("语言 Buffer 状态锁不应中毒")
            .file_path
            .clone()
    }

    /// 当前语言引用（编辑器输入行为等消费方取语言配置用，不克隆语法快照）。
    pub fn language(&self) -> Option<Arc<Language>> {
        self.state
            .lock()
            .expect("语言 Buffer 状态锁不应中毒")
            .syntax_map
            .language_arc()
    }

    pub fn language_name(&self) -> Option<&'static str> {
        let state = self.state.lock().expect("语言 Buffer 状态锁不应中毒");
        state.file_path.as_ref()?;
        state.syntax_map.language().map(Language::name)
    }

    pub fn set_file_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let text = self.buffer.read(cx).snapshot();
        let language_changed = {
            let mut state = self.state.lock().expect("语言 Buffer 状态锁不应中毒");
            advance_state(&mut state, &text, cx);
            let first_line = state.detection_first_line.clone();
            let language_changed =
                state
                    .syntax_map
                    .set_language_for_file(&path, Some(&first_line), &text);
            state.file_path = Some(path);
            if language_changed {
                state.settings = resolve_settings(&state.syntax_map, cx);
                state.highlight_cache = Arc::new(HighlightCache::new());
            }
            language_changed
        };
        if language_changed {
            self.start_reparse(cx);
        }
        cx.emit(LanguageBufferEvent::MetadataChanged);
        cx.notify();
    }

    /// 设置变化时刷新按语言解析的结果。
    fn refresh_settings(&mut self, cx: &mut Context<Self>) {
        let mut state = self.state.lock().expect("语言 Buffer 状态锁不应中毒");
        let settings = resolve_settings(&state.syntax_map, cx);
        if state.settings != settings {
            state.settings = settings;
            drop(state);
            cx.emit(LanguageBufferEvent::MetadataChanged);
            cx.notify();
        }
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let changes = self.subscription.consume();
        if changes.is_empty() {
            cx.emit(LanguageBufferEvent::MetadataChanged);
            cx.notify();
            return;
        }
        {
            let text = self.buffer.read(cx).snapshot();
            let mut state = self.state.lock().expect("语言 Buffer 状态锁不应中毒");
            advance_state(&mut state, &text, cx);
        }
        self.start_reparse(cx);
        cx.emit(LanguageBufferEvent::TextChanged);
        // 极快增量解析赶上当前按键：编辑轮内直接安装新鲜语法（见 install_sync_parse_result）。
        self.install_sync_parse_result(cx);
        cx.notify();
    }

    fn start_reparse(&mut self, cx: &mut Context<Self>) {
        // `ParseTask::drop` 会先通知 Tree-sitter 中止旧工作，再取消等待结果的前台任务。
        self.parse_task = None;
        let (text, syntax, registry) = {
            let state = self.state.lock().expect("语言 Buffer 状态锁不应中毒");
            if state
                .syntax_map
                .language()
                .and_then(Language::grammar)
                .is_none()
            {
                return;
            }
            (
                state.text_snapshot.clone(),
                state.syntax_map.snapshot(),
                state.syntax_map.registry(),
            )
        };

        let cancellation = ParseCancellation::default();
        let parse_cancellation = cancellation.clone();
        let completion: Arc<ParseCompletion> = Arc::default();
        let task_completion = Arc::clone(&completion);
        // 完成后置入完成信号：正在主线程短等待的 sync 可以直接同步安装新鲜语法。
        let parse_task = cx.background_spawn(async move {
            let outcome = syntax.reparse(&text, &registry, &parse_cancellation);
            let (lock, cvar) = &*task_completion;
            *lock.lock().expect("解析完成信号锁不应中毒") = outcome.clone();
            cvar.notify_one();
            outcome
        });
        let task = cx.spawn(async move |this, cx| {
            let Some(parsed) = parse_task.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                // 结果已被 sync 同步安装（parse_task 已替换为 None）时不再重复安装。
                this.parse_task = None;
                if this.install_parse_result(parsed, cx) {
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

    /// 主线程短等待后台解析：极快增量解析（通常远小于 1ms）赶上当前按键时，在编辑轮内直接安装新鲜语法，显示不停留在插值树（~1ms 同步解析预算；超时则保持原异步路径，稍后经 Reparsed 安装）。
    fn install_sync_parse_result(&mut self, cx: &mut Context<Self>) {
        let Some(parse_task) = self.parse_task.as_ref() else {
            return;
        };
        let Some(parsed) = parse_task.wait_completion(SYNC_PARSE_TIMEOUT) else {
            return;
        };
        if self.install_parse_result(parsed, cx) {
            // 结果已同步安装：丢弃异步安装路径（ParseTask::drop 取消后台任务）。
            self.parse_task = None;
        }
    }

    /// 唯一的解析安装入口：同步预算内完成与异步完成两条路径都经这里替换语法，并让高亮缓存整体失效。
    fn install_parse_result(&mut self, parsed: SyntaxSnapshot, cx: &mut Context<Self>) -> bool {
        let mut state = self.state.lock().expect("语言 Buffer 状态锁不应中毒");
        if !state.syntax_map.did_parse(parsed) {
            return false;
        }
        // 插值树被真实解析树替换：派生高亮整体失效。
        state.highlight_cache = Arc::new(HighlightCache::new());
        drop(state);
        cx.emit(LanguageBufferEvent::Reparsed);
        true
    }
}

/// 把语言状态推进到给定文本版本；文本版本变化时高亮缓存整体重建。
fn advance_state(state: &mut LanguageState, text: &Snapshot, cx: &App) {
    if state.text_snapshot.version() == text.version() {
        return;
    }
    state.syntax_map.interpolate(text);
    state.text_snapshot = text.clone();
    // 文本版本变化：高亮结果失效（对齐 Zed `Buffer::invalidate_tree_sitter_data`）。
    state.highlight_cache = Arc::new(HighlightCache::new());

    let next_first_line = first_line(text);
    if next_first_line != state.detection_first_line {
        state.detection_first_line = next_first_line;
        if let Some(path) = state.file_path.clone()
            && state.syntax_map.set_language_for_file(
                &path,
                Some(&state.detection_first_line),
                text,
            )
        {
            state.settings = resolve_settings(&state.syntax_map, cx);
        }
    }
}

fn resolve_settings(syntax_map: &SyntaxMap, cx: &App) -> Arc<LanguageSettings> {
    LanguageSettings::resolve(syntax_map.language().map(Language::name), cx)
}

fn first_line(snapshot: &Snapshot) -> String {
    snapshot
        .slice_line(Line::ZERO)
        .expect("文本快照始终至少包含第 0 行")
        .as_str()
        .trim_end_matches(['\r', '\n'])
        .to_owned()
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::TestAppContext;
    use zcv_text::{BufferConfig, BufferVersion, ByteOffset, Edit, TransactionMetadata};

    use super::*;

    fn test_registry() -> Arc<LanguageRegistry> {
        Arc::new(LanguageRegistry::new())
    }

    #[test]
    fn sync_parse_wait_returns_completed_result_within_timeout() {
        // 后台解析（真实线程）完成前主线程阻塞等待，完成后立即返回结果。
        let completion: Arc<ParseCompletion> = Arc::default();
        let worker = Arc::clone(&completion);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(5));
            let (lock, cvar) = &*worker;
            *lock.lock().expect("解析完成信号锁不应中毒") =
                Some(SyntaxSnapshot::empty(BufferVersion::INITIAL));
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
            Buffer::from_text("fn main() {}\n".to_owned(), BufferConfig::default())
                .expect("应创建测试 Buffer")
        });
        let language_buffer = cx.new(|cx| {
            LanguageBuffer::new(
                buffer.clone(),
                Some(PathBuf::from("main.rs")),
                test_registry(),
                cx,
            )
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

        let buffer_version = cx.read_entity(&buffer, |buffer, _| buffer.version());
        language_buffer.read_with(cx, |language_buffer, cx| {
            assert_eq!(
                language_buffer.snapshot(cx).syntax.version(),
                buffer_version
            );
        });
    }

    #[gpui::test]
    fn language_name_and_syntax_follow_first_line_changes(cx: &mut TestAppContext) {
        let buffer = cx.new(|_| {
            Buffer::from_text(String::new(), BufferConfig::default()).expect("应创建测试 Buffer")
        });
        let language_buffer = cx.new(|cx| {
            LanguageBuffer::new(
                buffer.clone(),
                Some(PathBuf::from("script")),
                test_registry(),
                cx,
            )
        });

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

        language_buffer.read_with(cx, |language_buffer, _cx| {
            assert_eq!(language_buffer.language_name(), Some("Python"));
        });
    }

    #[gpui::test]
    fn distinguishes_text_parse_and_metadata_events(cx: &mut TestAppContext) {
        let buffer = cx.new(|_| {
            Buffer::from_text("fn main() {}\n".to_owned(), BufferConfig::default())
                .expect("应创建测试 Buffer")
        });
        let language_buffer = cx.new(|cx| {
            LanguageBuffer::new(
                buffer.clone(),
                Some(PathBuf::from("main.rs")),
                test_registry(),
                cx,
            )
        });
        cx.run_until_parked();

        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = Rc::clone(&events);
        let _subscription = cx.update(|cx| {
            cx.subscribe(&language_buffer, move |_, event, _| {
                observed.borrow_mut().push(match event {
                    LanguageBufferEvent::TextChanged => "text",
                    LanguageBufferEvent::Reparsed => "reparsed",
                    LanguageBufferEvent::MetadataChanged => "metadata",
                });
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
        assert_eq!(events.borrow().as_slice(), ["text", "reparsed"]);

        events.borrow_mut().clear();
        buffer.update(cx, |buffer, cx| {
            buffer.mark_saved();
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(events.borrow().as_slice(), ["metadata"]);
    }

    #[gpui::test]
    fn text_event_wakes_consumers_after_language_snapshot_reaches_the_batch_version(
        cx: &mut TestAppContext,
    ) {
        let buffer = cx.new(|_| {
            Buffer::from_text("fn main() {}\n".to_owned(), BufferConfig::default())
                .expect("应创建测试 Buffer")
        });
        let direct_subscription = buffer.update(cx, |buffer, _| buffer.subscribe());
        let language_buffer = cx.new(|cx| {
            LanguageBuffer::new(
                buffer.clone(),
                Some(PathBuf::from("main.rs")),
                test_registry(),
                cx,
            )
        });
        cx.run_until_parked();

        let text_event_count = Rc::new(RefCell::new(0));
        let observed = Rc::clone(&text_event_count);
        let _subscription = cx.update(|cx| {
            cx.subscribe(&language_buffer, move |_, event, _| {
                if *event == LanguageBufferEvent::TextChanged {
                    *observed.borrow_mut() += 1;
                }
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

        let direct = direct_subscription.consume();
        assert_eq!(*text_event_count.borrow(), 1);
        assert!(direct.transaction_id().is_some());
        let language_snapshot = cx.read_entity(&language_buffer, |buffer, cx| buffer.snapshot(cx));
        assert_eq!(
            language_snapshot.text.version(),
            direct.new_version().expect("文本变化应有新版本")
        );
        assert_eq!(
            language_snapshot.syntax.version(),
            language_snapshot.text.version()
        );
    }

    #[gpui::test]
    /// 快速连续编辑：测试环境的后台任务由确定性调度驱动（不与主线程并发），
    /// sync 的 ~1ms 等待总是超时，中间解析被取消，只安装最新一次（1 次 Reparsed）。
    /// 生产环境（真实线程池）中每次编辑的极快解析会在编辑轮内同步安装，事件数可能更多，但任何时刻安装的语法都与当次文本版本一致。
    fn rapid_edits_install_only_the_latest_parse(cx: &mut TestAppContext) {
        let buffer = cx.new(|_| {
            Buffer::from_text("fn main() {}\n".to_owned(), BufferConfig::default())
                .expect("应创建测试 Buffer")
        });
        let language_buffer = cx.new(|cx| {
            LanguageBuffer::new(
                buffer.clone(),
                Some(PathBuf::from("main.rs")),
                test_registry(),
                cx,
            )
        });
        cx.run_until_parked();

        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = Rc::clone(&events);
        let _subscription = cx.update(|cx| {
            cx.subscribe(&language_buffer, move |_, event, _| {
                observed.borrow_mut().push(match event {
                    LanguageBufferEvent::TextChanged => "text",
                    LanguageBufferEvent::Reparsed => "reparsed",
                    LanguageBufferEvent::MetadataChanged => "metadata",
                });
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

        language_buffer.read_with(cx, |language_buffer, cx| {
            assert_eq!(
                language_buffer.snapshot(cx).syntax.version(),
                latest_version
            );
        });
        assert_eq!(
            events
                .borrow()
                .iter()
                .filter(|event| **event == "reparsed")
                .count(),
            1
        );
    }
}
