//! 五个测量场景：载入、解析、编辑、搜索、视口。
//!
//! 每个场景输出 `Measurement`：场景名、参数、迭代数、总耗时、平均耗时与备注。
//! 时间用 `Instant`，不做温度补偿；首次运行吃冷缓存，重复运行报告热缓存。

use std::fs;
use std::io::BufReader;
use std::path::Path;
use std::time::{Duration, Instant};

use std::sync::Arc;
use zom_engine::{
    Buffer, BufferConfig, BufferOrigin, ByteOffset, MetadataLayers, RegexSearchOptions, Viewport,
};
use zom_workspace::BufferId;
use zom_workspace::syntax::{
    BufferSyntaxState, HighlightProvider, HighlightSpan, LanguageId, SyntaxWorkerHandle, providers,
};

use crate::Lang;

#[derive(Debug, Clone)]
pub struct Measurement {
    pub scenario: &'static str,
    pub size_mib: usize,
    pub iters: u32,
    pub total: Duration,
    pub note: String,
}

impl Measurement {
    pub fn per_iter(&self) -> Duration {
        if self.iters == 0 {
            Duration::ZERO
        } else {
            self.total / self.iters
        }
    }
}

/// 读盘 + 解码 + 入 rope 的全链路（与 `Workspace::open_file` 一致：流式喂 ropey）。
pub fn measure_load(path: &Path, size_mib: usize) -> Measurement {
    const ITERS: u32 = 3;
    let mut total = Duration::ZERO;
    for _ in 0..ITERS {
        let t0 = Instant::now();
        let file = fs::File::open(path).expect("必须能打开基准语料文件");
        let reader = BufReader::with_capacity(64 * 1024, file);
        let origin = BufferOrigin::external(path.to_string_lossy().into_owned());
        let buffer = Buffer::from_reader(origin, reader, BufferConfig::default())
            .expect("基准语料必须能解码");
        total += t0.elapsed();
        drop(buffer);
    }
    Measurement {
        scenario: "load",
        size_mib,
        iters: ITERS,
        total,
        note: "Buffer::from_reader 流式：64 KiB 读缓冲 + 单次扫描合并 UTF-8/行尾/最长行".into(),
    }
}

/// 首次全量高亮解析的**端到端**耗时：attach 投任务 + worker 同步等待。
///
/// `BufferSyntaxState::attach` 现在异步返回，真正的 `run_full` 跑在后台线程。
/// 这里用 `worker.wait_for_idle()` 把 worker 计算时间也算进总耗时。
/// bench 关心的是「首屏高亮多久能看到」，等价于异步路径下的「冷启动延迟」。
pub fn measure_parse(buffer: &Buffer, lang: Lang, size_mib: usize) -> Option<Measurement> {
    let probe = make_provider(lang)?;
    let language = lang_id(lang);
    const ITERS: u32 = 3;
    let mut total = Duration::ZERO;
    for _ in 0..ITERS {
        let worker = Arc::new(SyntaxWorkerHandle::spawn());
        let mut layers = MetadataLayers::<HighlightSpan>::new();
        let provider: Box<dyn HighlightProvider> =
            make_provider(lang).expect("provider 已在前置探测中确认存在");
        let t0 = Instant::now();
        let state = BufferSyntaxState::attach(
            BufferId::from_raw(1),
            language,
            provider,
            buffer,
            &mut layers,
            worker.clone(),
            None,
        );
        worker.wait_for_idle();
        total += t0.elapsed();
        state.detach(&mut layers);
        worker.wait_for_idle();
    }
    drop(probe);
    Some(Measurement {
        scenario: "parse",
        size_mib,
        iters: ITERS,
        total,
        note: "attach 投任务 + worker wait_for_idle；端到端冷启动".into(),
    })
}

/// 单字符插入后**主线程**消耗。worker 的增量重解析不计入。
///
/// 红线指标：16 MiB rust 单键主线程时间 < 5 ms。
/// worker 端到端完成时间见 [`measure_edit_with_highlight_e2e`]。
pub fn measure_edit_with_highlight(
    buffer: &Buffer,
    lang: Lang,
    size_mib: usize,
) -> Option<Measurement> {
    // worker 不再阻塞主线程，单字符插入是常数代价。
    // 提高迭代数可以压低计时噪声。
    // 注意：iters 个任务会全部入队 worker。
    // 每条任务可能在后台跑数秒到数十秒。
    // 测完用 `forget_join` 脱离 worker，让进程立刻退出，不等积压跑完。
    let iters: u32 = match size_mib {
        1 => 5000,
        4 => 2000,
        16 => 500,
        _ => 200,
    };
    let language = lang_id(lang);
    let worker = Arc::new(SyntaxWorkerHandle::spawn());
    let mut layers = MetadataLayers::<HighlightSpan>::new();
    let provider = make_provider(lang)?;
    let mut buffer = buffer.clone();
    let state = BufferSyntaxState::attach(
        BufferId::from_raw(1),
        language,
        provider,
        &buffer,
        &mut layers,
        worker.clone(),
        None,
    );
    // 等首次全量解析落定，下一拍主线程发的编辑任务才能走增量。
    worker.wait_for_idle();
    let mid = ByteOffset::new(safe_insert_offset(&buffer));
    let _ = buffer.take_pending_events();

    let mut total = Duration::ZERO;
    for _ in 0..iters {
        let t0 = Instant::now();
        buffer.insert(mid, "x").expect("必须能插入单字符");
        let events = buffer.take_pending_events();
        let event = events.last().expect("插入必须产生事件");
        state.handle_edit(&buffer, event.changeset(), event.new_version(), &mut layers);
        total += t0.elapsed();
    }
    // 主线程时间测完即可。
    // worker 队列里可能还有上百个任务。
    // 每条 64 MiB 重解析任务真跑要数秒。
    // bench 不关心这些后台任务，脱离 worker 让进程退出时不 join。
    state.detach(&mut layers);
    worker.forget_join();

    Some(Measurement {
        scenario: "edit+hl main",
        size_mib,
        iters,
        total,
        note: "主线程：insert + take_pending + 投编辑任务；worker 重解析不计入".into(),
    })
}

/// 单字符插入后**端到端**完成时间（worker 把 spans 推到 sink 为止）。
///
/// 作为「单键端到端」对照值。主线程时间见
/// [`measure_edit_with_highlight`]。
pub fn measure_edit_with_highlight_e2e(
    buffer: &Buffer,
    lang: Lang,
    size_mib: usize,
) -> Option<Measurement> {
    let iters: u32 = match size_mib {
        1 => 50,
        4 => 10,
        _ => 3,
    };
    let language = lang_id(lang);
    let worker = Arc::new(SyntaxWorkerHandle::spawn());
    let mut layers = MetadataLayers::<HighlightSpan>::new();
    let provider = make_provider(lang)?;
    let mut buffer = buffer.clone();
    let state = BufferSyntaxState::attach(
        BufferId::from_raw(1),
        language,
        provider,
        &buffer,
        &mut layers,
        worker.clone(),
        None,
    );
    worker.wait_for_idle();
    let mid = ByteOffset::new(safe_insert_offset(&buffer));
    let _ = buffer.take_pending_events();

    let mut total = Duration::ZERO;
    for _ in 0..iters {
        let t0 = Instant::now();
        buffer.insert(mid, "x").expect("必须能插入单字符");
        let events = buffer.take_pending_events();
        let event = events.last().expect("插入必须产生事件");
        state.handle_edit(&buffer, event.changeset(), event.new_version(), &mut layers);
        worker.wait_for_idle();
        total += t0.elapsed();
    }
    state.detach(&mut layers);
    worker.wait_for_idle();

    Some(Measurement {
        scenario: "edit+hl e2e",
        size_mib,
        iters,
        total,
        note: "端到端：上一行 + worker wait_for_idle（增量重解析完成）".into(),
    })
}

/// 视口端到端：先把 viewport hint 设到中段附近 ±8 KiB。
/// 然后测单字符插入的端到端时间。
/// worker 在 hint 在线时只 query viewport 段。
/// spans 通过 `sink.replace_range` 投递。
/// 与 [`measure_edit_with_highlight_e2e`] 对照，直接测 viewport-scoped query 的收益。
///
/// **viewport-aware attach**：本 bench 把 viewport 直接作为 `initial_viewport`
/// 传给 attach——worker 在 attach 阶段就只对 viewport 段跑 query，不再先走全文
/// `run_full` 再 reissue。冷启动 `parse` 场景仍走 `None` 路径以维持"全文 query
/// 端到端"基线对照。
pub fn measure_edit_with_highlight_viewport(
    buffer: &Buffer,
    lang: Lang,
    size_mib: usize,
) -> Option<Measurement> {
    let iters: u32 = match size_mib {
        1 => 50,
        4 => 30,
        _ => 30,
    };
    let language = lang_id(lang);
    let worker = Arc::new(SyntaxWorkerHandle::spawn());
    let mut layers = MetadataLayers::<HighlightSpan>::new();
    let provider = make_provider(lang)?;
    let mut buffer = buffer.clone();

    // viewport ≈ mid ± 8 KiB——对应约 100 行 rust 可见区域 + 边界缓冲。在 attach
    // 之前就算出来，作为 initial_viewport 传入。
    let mid_byte = safe_insert_offset(&buffer);
    let mid = ByteOffset::new(mid_byte);
    let half = 8 * 1024;
    let total_bytes = buffer.snapshot().len_bytes().get();
    let vp_start = mid_byte.saturating_sub(half);
    let vp_end = (mid_byte + half).min(total_bytes);
    let viewport = zom_engine::TextRange::new(ByteOffset::new(vp_start), ByteOffset::new(vp_end))
        .expect("视口范围必须合法");

    let state = BufferSyntaxState::attach(
        BufferId::from_raw(1),
        language,
        provider,
        &buffer,
        &mut layers,
        worker.clone(),
        Some(viewport),
    );
    worker.wait_for_idle();

    let _ = buffer.take_pending_events();
    let mut total = Duration::ZERO;
    for _ in 0..iters {
        let t0 = Instant::now();
        buffer.insert(mid, "x").expect("必须能插入单字符");
        let events = buffer.take_pending_events();
        let event = events.last().expect("插入必须产生事件");
        state.handle_edit(&buffer, event.changeset(), event.new_version(), &mut layers);
        worker.wait_for_idle();
        total += t0.elapsed();
    }
    state.detach(&mut layers);
    worker.wait_for_idle();

    Some(Measurement {
        scenario: "edit+hl vp",
        size_mib,
        iters,
        total,
        note: "viewport ±8 KiB + worker wait_for_idle；viewport-scoped query + ReplaceRange".into(),
    })
}

pub fn measure_search(buffer: &Buffer, pattern: &str, size_mib: usize) -> Measurement {
    const ITERS: u32 = 3;
    let mut total = Duration::ZERO;
    let mut hits = 0usize;
    let mut error: Option<String> = None;
    for _ in 0..ITERS {
        let t0 = Instant::now();
        match buffer.search_regex(pattern, RegexSearchOptions::default()) {
            Ok(result) => {
                total += t0.elapsed();
                hits = result.len();
            }
            Err(e) => {
                total += t0.elapsed();
                error = Some(format!("{e}"));
                break;
            }
        }
    }
    let note = if let Some(e) = error {
        format!("regex \"{pattern}\"；引擎拒绝：{e}")
    } else {
        format!("regex \"{pattern}\"；命中 {hits} 处")
    };
    Measurement {
        scenario: "search",
        size_mib,
        iters: ITERS,
        total,
        note,
    }
}

pub fn measure_viewport(buffer: &Buffer, size_mib: usize) -> Measurement {
    const ITERS: u32 = 1000;
    let snapshot = buffer.snapshot();
    let total_lines = snapshot.line_count();
    let start_line_idx = total_lines / 2;
    let viewport = Viewport::new(zom_engine::Line::new(start_line_idx), 60);

    let mut total = Duration::ZERO;
    for _ in 0..ITERS {
        let t0 = Instant::now();
        let slice = snapshot.slice_viewport(viewport).expect("视口切片必须成功");
        total += t0.elapsed();
        let _ = slice.viewport();
    }
    Measurement {
        scenario: "viewport",
        size_mib,
        iters: ITERS,
        total,
        note: "snapshot.slice_viewport(60 行 @ 中位行)；衡量渲染热路径".into(),
    }
}

fn make_provider(lang: Lang) -> Option<Box<dyn HighlightProvider>> {
    match lang {
        Lang::Rust => Some(Box::new(providers::rust::new_provider())),
        Lang::Json => Some(Box::new(providers::json::new_provider())),
        Lang::Log => None,
    }
}

fn lang_id(lang: Lang) -> LanguageId {
    match lang {
        Lang::Rust => LanguageId::new("rust"),
        Lang::Json => LanguageId::new("json"),
        Lang::Log => LanguageId::new("plaintext"),
    }
}

/// 中段安全偏移：找到中间附近的换行符之后第一个字节，避免切到 UTF-8 多字节中间。
fn safe_insert_offset(buffer: &Buffer) -> usize {
    let snap = buffer.snapshot();
    let mid_line = snap.line_count() / 2;
    let line = snap
        .slice_line(zom_engine::Line::new(mid_line))
        .expect("中段行必须存在");
    line.range().start().get()
}
