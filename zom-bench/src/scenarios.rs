//! 五个测量场景：载入、解析、编辑、搜索、视口。
//!
//! 每个场景输出 `Measurement`：场景名、参数、迭代数、总耗时、平均耗时与备注。
//! 时间用 `Instant`，不做温度补偿；首次运行吃冷缓存，重复运行报告热缓存。

use std::fs;
use std::io::BufReader;
use std::path::Path;
use std::time::{Duration, Instant};

use std::sync::Arc;
use tree_sitter::{InputEdit, Language, Parser, Point, Query, QueryCursor, StreamingIterator};
use zom_engine::{Buffer, BufferConfig, BufferOrigin, ByteOffset, RegexSearchOptions, Viewport};
use zom_workspace::BufferId;
use zom_workspace::syntax::{
    BufferSyntax, HighlightProvider, LanguageId, SyntaxWorkerHandle, providers,
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

/// 带分位数的测量结果——给 render-time query / incremental reparse 这种"每帧都跑、关心尾延迟"的场景用。
/// p99 比平均值更能说明问题。
#[derive(Debug, Clone)]
pub struct PercentileMeasurement {
    pub scenario: &'static str,
    pub size_mib: usize,
    pub samples: Vec<Duration>,
    pub note: String,
}

impl PercentileMeasurement {
    fn percentile(&self, p: f64) -> Duration {
        if self.samples.is_empty() {
            return Duration::ZERO;
        }
        let mut sorted = self.samples.clone();
        sorted.sort();
        let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
        sorted[idx.min(sorted.len() - 1)]
    }
    pub fn p50(&self) -> Duration {
        self.percentile(0.50)
    }
    pub fn p95(&self) -> Duration {
        self.percentile(0.95)
    }
    pub fn p99(&self) -> Duration {
        self.percentile(0.99)
    }
    pub fn count(&self) -> usize {
        self.samples.len()
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
/// `BufferSyntax::attach` 异步返回，真正的 `run_full` 跑在后台线程。
/// 这里用 `worker.wait_for_idle()` 把 worker 计算时间也算进总耗时。
/// bench 关心的是「首屏高亮多久能看到」，等价于异步路径下的「冷启动延迟」。
pub fn measure_parse(buffer: &Buffer, lang: Lang, size_mib: usize) -> Option<Measurement> {
    let probe = make_provider(lang)?;
    let language = lang_id(lang);
    const ITERS: u32 = 3;
    let mut total = Duration::ZERO;
    for _ in 0..ITERS {
        let worker = Arc::new(SyntaxWorkerHandle::spawn());
        let provider: Box<dyn HighlightProvider> =
            make_provider(lang).expect("provider 已在前置探测中确认存在");
        let t0 = Instant::now();
        let state = BufferSyntax::attach(
            BufferId::from_raw(1),
            language,
            provider,
            buffer,
            worker.clone(),
        );
        worker.wait_for_idle();
        total += t0.elapsed();
        state.detach();
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
    let provider = make_provider(lang)?;
    let mut buffer = buffer.clone();
    let state = BufferSyntax::attach(
        BufferId::from_raw(1),
        language,
        provider,
        &buffer,
        worker.clone(),
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
        state.handle_edit(&buffer, event);
        total += t0.elapsed();
    }
    // 主线程时间测完即可。bench 不关心后台任务，脱离 worker 让进程退出时不 join。
    state.detach();
    worker.forget_join();

    Some(Measurement {
        scenario: "edit+hl main",
        size_mib,
        iters,
        total,
        note: "主线程：insert + take_pending + 同步 tree.edit + 投编辑任务".into(),
    })
}

/// 单字符插入后**端到端**完成时间（worker 把 spans 推到 sink 为止）。
///
/// 作为「单键端到端」对照值。主线程时间见 [`measure_edit_with_highlight`]。
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
    let provider = make_provider(lang)?;
    let mut buffer = buffer.clone();
    let state = BufferSyntax::attach(
        BufferId::from_raw(1),
        language,
        provider,
        &buffer,
        worker.clone(),
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
        state.handle_edit(&buffer, event);
        worker.wait_for_idle();
        total += t0.elapsed();
    }
    state.detach();
    worker.wait_for_idle();

    Some(Measurement {
        scenario: "edit+hl e2e",
        size_mib,
        iters,
        total,
        note: "端到端：上一行 + worker wait_for_idle（增量重解析完成）".into(),
    })
}

// Phase 3 移除 `measure_edit_with_highlight_viewport` —— worker 不再有 viewport hint 概念（viewport-scoped Query 由 paint 阶段从共享 `BufferSyntaxTree` 现做）；
// 端到端 edit 仍走 [`measure_edit_with_highlight_e2e`]。

pub fn measure_search(buffer: &Buffer, pattern: &str, size_mib: usize) -> Measurement {
    const ITERS: u32 = 3;
    let mut total = Duration::ZERO;
    let mut hits = 0usize;
    let mut error: Option<String> = None;
    for _ in 0..ITERS {
        let t0 = Instant::now();
        match buffer
            .snapshot()
            .search_regex(pattern, RegexSearchOptions::default())
            .join()
        {
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

// =============================================================================
// Phase 0：render-time query 改造的前置测量。
//
// 两个场景都直接用 tree-sitter 原始 API（绕开 HighlightProvider 抽象），目的是让数字尽量接近 B 架构（render-time query + sync incremental reparse）真实形态：
//
// - `measure_render_query`：模拟"每帧 paint 时按 viewport 现查"。
// - `measure_incremental_reparse`：模拟"每次 edit 主线程同步 tree.edit + reparse"。
//
// 两者只跑 rust 16 MiB（红线档）；其它规模与语言留到改造落地后再补。
// =============================================================================

const QUERY_SAMPLES: usize = 200; // viewport query 样本数：足以稳出 p99，不到 1 秒
const REPARSE_SAMPLES: usize = 100; // incremental reparse 样本数：单次 5-50ms 量级
const VIEWPORT_LINES: usize = 50; // ~50 行 ≈ 2-4 KiB 典型 rust 代码视口

/// 模拟 render-time query：对随机 ~50 行视口跑 `QueryCursor::captures`，全链路含 spans 物化。
/// tree 是预先全文 parse 后的稳态——只测 query 本身的开销，不算 reparse。
///
/// 对应改造计划 Phase 0 决策点：
/// - p99 < 0.5 ms → render-time 不缓存。
/// - p99 0.5-2 ms → 加 viewport spans cache。
/// - p99 > 2 ms → 先优化 query 再开工。
pub fn measure_render_query(
    buffer: &Buffer,
    lang: Lang,
    size_mib: usize,
) -> Option<PercentileMeasurement> {
    // 只支持有 tree-sitter grammar 的语言；
    // 这里先只接 rust，与 Phase 0 决策对齐。
    if !matches!(lang, Lang::Rust) {
        return None;
    }
    let language: Language = tree_sitter_rust::LANGUAGE.into();
    let query = Query::new(&language, tree_sitter_rust::HIGHLIGHTS_QUERY)
        .expect("rust query 必须 build 成功");
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .expect("rust grammar ABI 必须匹配");

    // 一次性物化全文：bench 关心 query 本身耗时，不算文本物化开销。
    let snapshot = buffer.snapshot();
    let text = snapshot
        .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
        .expect("全文切片必须成功")
        .into_text()
        .into_owned();
    let bytes = text.as_bytes();
    let tree = parser.parse(bytes, None).expect("全文 parse 必须成功");

    // 预算 200 个随机 viewport：
    // 用 LCG 撒在中间 80% 行范围里，避免落到文件首尾的稀疏区。
    let total_lines = snapshot.line_count();
    let lo = total_lines / 10;
    let hi = total_lines - total_lines / 10 - VIEWPORT_LINES;
    let mut rng_state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut viewports: Vec<(usize, usize)> = Vec::with_capacity(QUERY_SAMPLES);
    for _ in 0..QUERY_SAMPLES {
        rng_state = rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let span = (hi - lo).max(1) as u64;
        let start_line = lo + ((rng_state >> 33) % span) as usize;
        let end_line = (start_line + VIEWPORT_LINES).min(total_lines - 1);
        let start_byte = snapshot
            .slice_line(zom_engine::Line::new(start_line))
            .expect("行必须存在")
            .range()
            .start()
            .get();
        let end_byte = snapshot
            .slice_line(zom_engine::Line::new(end_line))
            .expect("行必须存在")
            .range()
            .end()
            .get();
        viewports.push((start_byte, end_byte));
    }

    let mut cursor = QueryCursor::new();
    let mut samples: Vec<Duration> = Vec::with_capacity(QUERY_SAMPLES);
    // 预热：cursor 内部 capture state machine 第一次走比稳态慢，丢掉头 5 个样本。
    for &(s, e) in viewports.iter().take(5) {
        cursor.set_byte_range(s..e);
        let mut captures = cursor.captures(&query, tree.root_node(), bytes);
        while captures.next().is_some() {}
    }
    for &(s, e) in &viewports {
        let t0 = Instant::now();
        cursor.set_byte_range(s..e);
        let mut captures = cursor.captures(&query, tree.root_node(), bytes);
        // 模拟 collect_spans：流式吃完所有 capture 事件，让分位数包含完整链路。
        let mut count: usize = 0;
        while let Some((_, _)) = captures.next() {
            count += 1;
        }
        samples.push(t0.elapsed());
        // 防优化掉
        std::hint::black_box(count);
    }

    Some(PercentileMeasurement {
        scenario: "render-time query",
        size_mib,
        samples,
        note: format!(
            "rust grammar + HIGHLIGHTS_QUERY；{VIEWPORT_LINES} 行 viewport ×{QUERY_SAMPLES} 次，整段全文 bytes"
        ),
    })
}

/// 模拟主线程同步增量 reparse：每次插入单字符，调 `tree.edit` 后跑 `Parser::parse_with_options(..., Some(&old_tree), timeout=None)`。
///
/// 对应改造计划 Phase 0：决定是否在 B 架构下叠加"主线程同步 reparse fast-path"。
pub fn measure_incremental_reparse(
    buffer: &Buffer,
    lang: Lang,
    size_mib: usize,
) -> Option<PercentileMeasurement> {
    if !matches!(lang, Lang::Rust) {
        return None;
    }
    let language: Language = tree_sitter_rust::LANGUAGE.into();
    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .expect("rust grammar ABI 必须匹配");

    let snapshot = buffer.snapshot();
    let text = snapshot
        .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
        .expect("全文切片必须成功")
        .into_text()
        .into_owned();
    let mut bytes = text.into_bytes();
    let mut tree = parser.parse(&bytes, None).expect("首次 parse 必须成功");

    // 在中段一个稳定的行起点插入：
    // 每次插入都在同一 byte offset，column 永远是 0（我们插的是 ASCII 'x'），
    // 不跨行——这样 InputEdit 的 Point 计算简单稳定。
    let mid_byte = safe_insert_offset(buffer);
    let mid_line = snapshot.line_count() / 2;

    let mut samples: Vec<Duration> = Vec::with_capacity(REPARSE_SAMPLES);
    // 预热 3 次：parser/incremental cache 第一次比稳态慢。
    for _ in 0..3 {
        let edit = build_input_edit(mid_byte, mid_line);
        bytes.insert(mid_byte, b'x');
        tree.edit(&edit);
        tree = parser
            .parse(&bytes, Some(&tree))
            .expect("增量 reparse 必须成功");
    }
    for _ in 0..REPARSE_SAMPLES {
        let edit = build_input_edit(mid_byte, mid_line);
        bytes.insert(mid_byte, b'x');
        let t0 = Instant::now();
        tree.edit(&edit);
        tree = parser
            .parse(&bytes, Some(&tree))
            .expect("增量 reparse 必须成功");
        samples.push(t0.elapsed());
        std::hint::black_box(tree.root_node().end_byte());
    }

    Some(PercentileMeasurement {
        scenario: "incremental reparse",
        size_mib,
        samples,
        note: format!(
            "rust grammar；单字符 ASCII 插入 @ 中段行首 ×{REPARSE_SAMPLES} 次；tree.edit + parse_with_options(Some(&old))"
        ),
    })
}

/// 给"在固定 byte offset 插入单个 ASCII 字符"构造 tree-sitter InputEdit。
/// 该 offset 是某一行的起点；插入 'x' 后下一行起点会向后挪 1 字节，但下次插入位置不变。
fn build_input_edit(byte_offset: usize, row: usize) -> InputEdit {
    InputEdit {
        start_byte: byte_offset,
        old_end_byte: byte_offset,
        new_end_byte: byte_offset + 1,
        start_position: Point::new(row, 0),
        old_end_position: Point::new(row, 0),
        new_end_position: Point::new(row, 1),
    }
}
