use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use gpui::{App, AppContext, Context, EventEmitter, Task};
use zcv_settings::SettingsStore;
use zcv_text::{
    Buffer, BufferVersion, ByteOffset, Edit, HistoryEditOutcome, Line, Snapshot, TextResult,
    TextSubscription, TransactionId, TransactionMetadata, TransactionOutcome,
};

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
    syntax_map: SyntaxMap,
    detection_first_line: String,
    file_path: Option<PathBuf>,
    settings: Arc<LanguageSettings>,
    highlight_cache: Arc<HighlightCache>,
}

/// 一个打开文档的唯一权威实体：直接拥有文本 Buffer 与树语法状态。
///
/// 文本与语法同属一个实体，snapshot() 返回同一版本的二者；
/// 语言层不暴露内层文本实体，也不维护第二份派生文本快照。
/// 多个 Editor 可以共享一个 LanguageBuffer，后台也只会存在一个解析任务。
pub struct LanguageBuffer {
    buffer: Buffer,
    state: Mutex<LanguageState>,
    parse_task: Option<ParseTask>,
}

impl LanguageBuffer {
    pub fn new(
        buffer: Buffer,
        file_path: Option<PathBuf>,
        registry: Arc<LanguageRegistry>,
        cx: &mut Context<Self>,
    ) -> Self {
        let snapshot = buffer.snapshot();
        let first_line = first_line(&snapshot);
        let mut syntax_map = SyntaxMap::new(Arc::clone(&registry), &snapshot);
        if let Some(path) = file_path.as_deref() {
            syntax_map.set_language_for_file(path, Some(&first_line), &snapshot);
        }
        let settings = resolve_settings(&syntax_map, cx);

        // 设置变化时刷新按语言解析的结果；测试环境未注册 SettingsStore 时不建立订阅。
        if cx.try_global::<SettingsStore>().is_some() {
            cx.observe_global::<SettingsStore>(|language_buffer, cx| {
                language_buffer.refresh_settings(cx);
            })
            .detach();
        }

        let mut this = Self {
            buffer,
            state: Mutex::new(LanguageState {
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

    /// 本 Buffer 使用的语言注册表；需要创建关联语言 Buffer 的消费方复用同一份注册表。
    pub fn language_registry(&self) -> Arc<LanguageRegistry> {
        self.state
            .lock()
            .expect("语言 Buffer 状态锁不应中毒")
            .syntax_map
            .registry()
    }

    /// 一致性快照：返回前把语法状态推进到文本版本，文本与语法属于同一 BufferVersion。
    pub fn snapshot(&self) -> LanguageBufferSnapshot {
        let text = self.buffer.snapshot();
        let mut state = self.state.lock().expect("语言 Buffer 状态锁不应中毒");
        state.syntax_map.interpolate(&text);
        LanguageBufferSnapshot {
            text,
            syntax: state.syntax_map.snapshot(),
            language: state.syntax_map.language_arc(),
            settings: Arc::clone(&state.settings),
            file_path: state.file_path.clone(),
            highlight_cache: Arc::clone(&state.highlight_cache),
        }
    }

    /// 同源只读文本快照：直接取自唯一权威文本，不复制第二份文本。
    ///
    /// 需要文本与语法一致版本的消费方应使用 Self::snapshot。
    pub fn text_snapshot(&self) -> Snapshot {
        self.buffer.snapshot()
    }

    /// 订阅本 Buffer 的版本化文本增量；语言层只转发权威文本的订阅，不维护第二份变更游标。
    pub fn subscribe(&self) -> TextSubscription {
        self.buffer.subscribe()
    }

    /// 当前文本版本。
    pub fn version(&self) -> BufferVersion {
        self.buffer.version()
    }

    /// 当前文本字节长度。
    pub fn len_bytes(&self) -> ByteOffset {
        self.buffer.len_bytes()
    }

    /// 自保存点以来是否存在结构性文本编辑。
    pub fn is_dirty(&self) -> bool {
        self.buffer.is_dirty()
    }

    /// 当前历史节点的事务身份；无历史时为 None。
    pub fn current_history_transaction_id(&self) -> Option<TransactionId> {
        self.buffer.current_history_transaction_id()
    }

    pub fn can_undo(&self) -> bool {
        self.buffer.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.buffer.can_redo()
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
        let text = self.buffer.snapshot();
        let language_changed = {
            let mut state = self.state.lock().expect("语言 Buffer 状态锁不应中毒");
            state.syntax_map.interpolate(&text);
            let first_line = first_line(&text);
            state.detection_first_line = first_line.clone();
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

    /// 应用一个本地编辑批次；委托权威文本提交后推进语法插值、解析与事件。
    pub fn edit(
        &mut self,
        edits: impl IntoIterator<Item = Edit>,
        metadata: TransactionMetadata,
        cx: &mut Context<Self>,
    ) -> TextResult<TransactionOutcome> {
        let before = self.buffer.version();
        let outcome = self.buffer.edit(edits, metadata)?;
        if self.buffer.version() != before {
            self.did_edit(cx);
        }
        Ok(outcome)
    }

    /// 用外部文本整体重置文本；文本变化时推进语法与事件，文本相同时只刷新保存点。
    pub fn reset(&mut self, text: String, cx: &mut Context<Self>) -> TextResult<()> {
        let before = self.buffer.version();
        self.buffer.reset(text)?;
        if self.buffer.version() != before {
            self.did_edit(cx);
        } else {
            cx.emit(LanguageBufferEvent::MetadataChanged);
            cx.notify();
        }
        Ok(())
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) -> TextResult<Option<HistoryEditOutcome>> {
        let outcome = self.buffer.undo()?;
        if outcome.is_some() {
            self.did_edit(cx);
        }
        Ok(outcome)
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) -> TextResult<Option<HistoryEditOutcome>> {
        let outcome = self.buffer.redo()?;
        if outcome.is_some() {
            self.did_edit(cx);
        }
        Ok(outcome)
    }

    pub fn start_transaction(&mut self) -> TextResult<Option<TransactionId>> {
        self.buffer.start_transaction()
    }

    pub fn end_transaction(&mut self) -> TextResult<Option<TransactionId>> {
        self.buffer.end_transaction()
    }

    /// 标记当前版本为保存点；保存点是元数据变化，不推进语法。
    pub fn mark_saved(&mut self, cx: &mut Context<Self>) {
        self.buffer.mark_saved();
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

    /// 文本提交后推进语言状态：插值到新版本、失效高亮、必要时重检测语言并调度解析。
    fn did_edit(&mut self, cx: &mut Context<Self>) {
        let text = self.buffer.snapshot();
        {
            let mut state = self.state.lock().expect("语言 Buffer 状态锁不应中毒");
            state.syntax_map.interpolate(&text);
            // 文本版本变化：高亮结果整体失效（对齐 Zed 的 Buffer::invalidate_tree_sitter_data）。
            state.highlight_cache = Arc::new(HighlightCache::new());

            let next_first_line = first_line(&text);
            if next_first_line != state.detection_first_line {
                state.detection_first_line = next_first_line;
                let detection_first_line = state.detection_first_line.clone();
                if let Some(path) = state.file_path.clone()
                    && state.syntax_map.set_language_for_file(
                        &path,
                        Some(&detection_first_line),
                        &text,
                    )
                {
                    state.settings = resolve_settings(&state.syntax_map, cx);
                }
            }
        }
        self.start_reparse(cx);
        cx.emit(LanguageBufferEvent::TextChanged);
        // 极快增量解析赶上当前按键：编辑轮内直接安装新鲜语法（见 install_sync_parse_result）。
        self.install_sync_parse_result(cx);
        cx.notify();
    }

    fn start_reparse(&mut self, cx: &mut Context<Self>) {
        // ParseTask::drop 会先通知 Tree-sitter 中止旧工作，再取消等待结果的前台任务。
        self.parse_task = None;
        let text = self.buffer.snapshot();
        let (syntax, registry) = {
            let state = self.state.lock().expect("语言 Buffer 状态锁不应中毒");
            if state
                .syntax_map
                .language()
                .and_then(Language::grammar)
                .is_none()
            {
                return;
            }
            (state.syntax_map.snapshot(), state.syntax_map.registry())
        };

        let cancellation = ParseCancellation::default();
        let parse_cancellation = cancellation.clone();
        let completion: Arc<ParseCompletion> = Arc::default();
        let task_completion = Arc::clone(&completion);
        // 完成后置入完成信号：正在主线程短等待的 did_edit 可以直接同步安装新鲜语法。
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
                // 结果已被 did_edit 同步安装（parse_task 已替换为 None）时不再重复安装。
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
    use zcv_text::{BufferConfig, Edit, TransactionMetadata};

    use super::*;

    fn test_registry() -> Arc<LanguageRegistry> {
        Arc::new(LanguageRegistry::new())
    }

    fn test_buffer(text: &str) -> Buffer {
        Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("应创建测试 Buffer")
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
        let language_buffer = cx.new(|cx| {
            LanguageBuffer::new(
                test_buffer("fn main() {}\n"),
                Some(PathBuf::from("main.rs")),
                test_registry(),
                cx,
            )
        });

        language_buffer.update(cx, |language_buffer, cx| {
            language_buffer
                .edit(
                    [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                    TransactionMetadata::default(),
                    cx,
                )
                .expect("测试编辑应成功");
        });
        cx.run_until_parked();

        language_buffer.read_with(cx, |language_buffer, _| {
            let snapshot = language_buffer.snapshot();
            assert_eq!(snapshot.syntax.version(), snapshot.text.version());
        });
    }

    #[gpui::test]
    fn language_name_and_syntax_follow_first_line_changes(cx: &mut TestAppContext) {
        let language_buffer = cx.new(|cx| {
            LanguageBuffer::new(
                test_buffer(""),
                Some(PathBuf::from("script")),
                test_registry(),
                cx,
            )
        });

        language_buffer.read_with(cx, |language_buffer, _| {
            // 未识别文件以纯文本兜底，且无语法树。
            assert_eq!(language_buffer.language_name(), Some("纯文本"));
            let language = language_buffer.language().expect("兜底语言应存在");
            assert_eq!(language.name(), "纯文本");
            assert!(language.grammar().is_none(), "纯文本兜底不应有语法树");
            assert!(
                language_buffer.parse_task.is_none(),
                "纯文本不应启动无意义的后台解析任务"
            );
        });
        language_buffer.update(cx, |language_buffer, cx| {
            language_buffer
                .edit(
                    [
                        Edit::insert(ByteOffset::ZERO, "#!/usr/bin/env python\nprint('ok')\n")
                            .unwrap(),
                    ],
                    TransactionMetadata::default(),
                    cx,
                )
                .expect("测试编辑应成功");
        });
        cx.run_until_parked();

        language_buffer.read_with(cx, |language_buffer, _cx| {
            assert_eq!(language_buffer.language_name(), Some("Python"));
        });
    }

    #[gpui::test]
    fn distinguishes_text_parse_and_metadata_events(cx: &mut TestAppContext) {
        let language_buffer = cx.new(|cx| {
            LanguageBuffer::new(
                test_buffer("fn main() {}\n"),
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

        language_buffer.update(cx, |language_buffer, cx| {
            language_buffer
                .edit(
                    [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                    TransactionMetadata::default(),
                    cx,
                )
                .expect("测试编辑应成功");
        });
        cx.run_until_parked();
        assert_eq!(events.borrow().as_slice(), ["text", "reparsed"]);

        events.borrow_mut().clear();
        language_buffer.update(cx, |language_buffer, cx| {
            language_buffer.mark_saved(cx);
        });
        cx.run_until_parked();
        assert_eq!(events.borrow().as_slice(), ["metadata"]);
    }

    #[gpui::test]
    fn text_event_wakes_consumers_after_language_snapshot_reaches_the_batch_version(
        cx: &mut TestAppContext,
    ) {
        let language_buffer = cx.new(|cx| {
            LanguageBuffer::new(
                test_buffer("fn main() {}\n"),
                Some(PathBuf::from("main.rs")),
                test_registry(),
                cx,
            )
        });
        cx.run_until_parked();

        let direct_subscription =
            language_buffer.read_with(cx, |language_buffer, _| language_buffer.subscribe());
        let text_event_count = Rc::new(RefCell::new(0));
        let observed = Rc::clone(&text_event_count);
        let _subscription = cx.update(|cx| {
            cx.subscribe(&language_buffer, move |_, event, _| {
                if *event == LanguageBufferEvent::TextChanged {
                    *observed.borrow_mut() += 1;
                }
            })
        });

        language_buffer.update(cx, |language_buffer, cx| {
            language_buffer
                .edit(
                    [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                    TransactionMetadata::default(),
                    cx,
                )
                .expect("测试编辑应成功");
        });
        cx.run_until_parked();

        let direct = direct_subscription.consume();
        assert_eq!(*text_event_count.borrow(), 1);
        assert!(direct.transaction_id().is_some());
        let language_snapshot = cx.read_entity(&language_buffer, |buffer, _| buffer.snapshot());
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
    fn rapid_edits_install_only_the_latest_parse(cx: &mut TestAppContext) {
        let language_buffer = cx.new(|cx| {
            LanguageBuffer::new(
                test_buffer("fn main() {}\n"),
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
            language_buffer.update(cx, |language_buffer, cx| {
                let offset = language_buffer.len_bytes();
                language_buffer
                    .edit(
                        [Edit::insert(offset, text).unwrap()],
                        TransactionMetadata::default(),
                        cx,
                    )
                    .expect("测试编辑应成功");
            });
        }
        let latest_version =
            language_buffer.read_with(cx, |language_buffer, _| language_buffer.version());
        cx.run_until_parked();

        language_buffer.read_with(cx, |language_buffer, _| {
            assert_eq!(language_buffer.snapshot().syntax.version(), latest_version);
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

    /// 回归：文本与语法在同一实体上编辑后必须停留在同一版本，不再依赖跨实体观察者。
    #[gpui::test]
    fn text_and_syntax_share_the_version_after_same_entity_edit(cx: &mut TestAppContext) {
        let language_buffer = cx.new(|cx| {
            LanguageBuffer::new(
                test_buffer("fn main() {}\n"),
                Some(PathBuf::from("main.rs")),
                test_registry(),
                cx,
            )
        });
        language_buffer.update(cx, |language_buffer, cx| {
            language_buffer
                .edit(
                    [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                    TransactionMetadata::default(),
                    cx,
                )
                .expect("测试编辑应成功");
        });
        cx.run_until_parked();

        language_buffer.read_with(cx, |language_buffer, _| {
            let snapshot = language_buffer.snapshot();
            assert_eq!(snapshot.text.version(), language_buffer.version());
            assert_eq!(snapshot.syntax.version(), snapshot.text.version());
        });
    }

    /// 回归：LanguageBuffer 仍需暴露与持有文本同源的版本化增量，供组合层惰性拉取。
    #[gpui::test]
    fn versioned_incremental_batch_is_still_available(cx: &mut TestAppContext) {
        let language_buffer = cx.new(|cx| {
            LanguageBuffer::new(
                test_buffer("fn main() {}\n"),
                Some(PathBuf::from("main.rs")),
                test_registry(),
                cx,
            )
        });
        let subscription =
            language_buffer.read_with(cx, |language_buffer, _| language_buffer.subscribe());
        let old_version =
            language_buffer.read_with(cx, |language_buffer, _| language_buffer.version());

        language_buffer.update(cx, |language_buffer, cx| {
            language_buffer
                .edit(
                    [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                    TransactionMetadata::default(),
                    cx,
                )
                .expect("测试编辑应成功");
        });

        let changes = subscription.consume();
        assert!(!changes.is_empty(), "应能拉取到版本化增量");
        assert_eq!(changes.old_version(), Some(old_version));
        assert!(changes.new_version().is_some());
    }
}
