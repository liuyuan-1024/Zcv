//! `zom-bench` —— 16 MiB 目标的基线测量工具。
//!
//! 用法（一律 release 跑）：
//!
//! ```text
//! cargo run --release -p zom-bench -- corpus           # 生成 1/4/16 MiB 语料到 target/bench-corpus/
//! cargo run --release -p zom-bench -- run rust         # 跑 rust 全部规模
//! cargo run --release -p zom-bench -- run rust 16      # 只跑 rust 16 MiB 一档（红线档）
//! cargo run --release -p zom-bench -- run all          # 三语言全跑
//! ```
//!
//! 内存峰值不在本工具内测量。
//! 建议在外层包一层 `/usr/bin/time -l`（macOS）或 `/usr/bin/time -v`（Linux），读取最大常驻集大小。

mod corpus;
mod scenarios;

use std::env;
use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use zom_engine::{Buffer, BufferConfig, BufferOrigin};

use scenarios::{Measurement, PercentileMeasurement};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    Json,
    Log,
}

impl Lang {
    pub fn name(self) -> &'static str {
        match self {
            Lang::Rust => "rust",
            Lang::Json => "json",
            Lang::Log => "log",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            Lang::Rust => "rs",
            Lang::Json => "json",
            Lang::Log => "log",
        }
    }
    pub fn search_pattern(self) -> &'static str {
        match self {
            Lang::Rust => r"\bfn\b|\blet\b|\bimpl\b",
            Lang::Json => r#""id":\s*\d+"#,
            Lang::Log => r"ERROR|WARN",
        }
    }
    pub fn from_arg(s: &str) -> Option<Lang> {
        match s {
            "rust" => Some(Lang::Rust),
            "json" => Some(Lang::Json),
            "log" => Some(Lang::Log),
            _ => None,
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");

    match cmd {
        "corpus" => match corpus::ensure_all() {
            Ok(()) => {
                println!("语料已就绪：{}", corpus::corpus_dir().display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("语料生成失败：{e}");
                ExitCode::FAILURE
            }
        },
        "load" => {
            if let Err(e) = corpus::ensure_all() {
                eprintln!("语料生成失败：{e}");
                return ExitCode::FAILURE;
            }
            let lang_arg = args.get(1).map(String::as_str).unwrap_or("rust");
            let size_arg: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(16);
            let Some(lang) = Lang::from_arg(lang_arg) else {
                eprintln!("未知语言：{lang_arg}");
                return ExitCode::FAILURE;
            };
            let path = corpus::fixture_path(lang, size_arg);
            // 单次载入并持有 buffer 直到 main 退出。
            // 外挂 time 取到的 peak RSS 就是仅载入峰值，不会被解析 / spans 污染。
            let buffer = load_buffer(&path);
            println!(
                "已载入 {} {} MiB → {} 字节，{} 行",
                lang.name(),
                size_arg,
                buffer.len_bytes().get(),
                buffer.snapshot().line_count()
            );
            // 故意不 drop，让外层 time 看到载入完成后的稳态 RSS。
            std::mem::forget(buffer);
            ExitCode::SUCCESS
        }
        "run" => {
            if let Err(e) = corpus::ensure_all() {
                eprintln!("语料生成失败：{e}");
                return ExitCode::FAILURE;
            }
            let lang_arg = args.get(1).map(String::as_str).unwrap_or("all");
            let size_filter: Option<usize> = args.get(2).and_then(|s| s.parse().ok());
            let langs: Vec<Lang> = if lang_arg == "all" {
                vec![Lang::Rust, Lang::Json, Lang::Log]
            } else if let Some(l) = Lang::from_arg(lang_arg) {
                vec![l]
            } else {
                eprintln!("未知语言：{lang_arg}（rust / json / log / all）");
                return ExitCode::FAILURE;
            };

            let mut rows: Vec<Measurement> = Vec::new();
            let mut percentile_rows: Vec<PercentileMeasurement> = Vec::new();
            for lang in langs {
                for &mib in corpus::sizes() {
                    if let Some(filter) = size_filter
                        && filter != mib
                    {
                        continue;
                    }
                    let path = corpus::fixture_path(lang, mib);
                    eprintln!("== {} {} MiB ==（{}）", lang.name(), mib, path.display());
                    run_for_fixture(lang, mib, &path, &mut rows, &mut percentile_rows);
                }
            }
            print_table(&rows);
            print_percentile_table(&percentile_rows);
            ExitCode::SUCCESS
        }
        _ => {
            print_usage();
            ExitCode::SUCCESS
        }
    }
}

fn run_for_fixture(
    lang: Lang,
    mib: usize,
    path: &Path,
    rows: &mut Vec<Measurement>,
    percentile_rows: &mut Vec<PercentileMeasurement>,
) {
    let load = scenarios::measure_load(path, mib);
    eprintln!("  load        {}", fmt_dur(load.per_iter()));
    rows.push(load);

    let buffer = load_buffer(path);

    let view = scenarios::measure_viewport(&buffer, mib);
    eprintln!("  viewport    {}", fmt_dur(view.per_iter()));
    rows.push(view);

    let search = scenarios::measure_search(&buffer, lang.search_pattern(), mib);
    if search.note.contains("引擎拒绝") {
        eprintln!("  search      {} (引擎拒绝)", fmt_dur(search.per_iter()));
    } else {
        eprintln!("  search      {}", fmt_dur(search.per_iter()));
    }
    rows.push(search);

    if let Some(parse) = scenarios::measure_parse(&buffer, lang, mib) {
        eprintln!("  parse       {}", fmt_dur(parse.per_iter()));
        rows.push(parse);
    } else {
        eprintln!("  parse       跳过（{} 没有高亮提供器）", lang.name());
    }

    if let Some(edit) = scenarios::measure_edit_with_highlight(&buffer, lang, mib) {
        eprintln!(
            "  edit+hl m   {}/edit (×{})  (主线程)",
            fmt_dur(edit.per_iter()),
            edit.iters
        );
        rows.push(edit);
    } else {
        eprintln!("  edit+hl m   跳过（{} 没有高亮提供器）", lang.name());
    }

    if let Some(edit) = scenarios::measure_edit_with_highlight_e2e(&buffer, lang, mib) {
        eprintln!(
            "  edit+hl e   {}/edit (×{})  (端到端，全文 query)",
            fmt_dur(edit.per_iter()),
            edit.iters
        );
        rows.push(edit);
    } else {
        eprintln!("  edit+hl e   跳过（{} 没有高亮提供器）", lang.name());
    }

    // Phase 0：render-time query 改造的前置数字。只跑 rust。
    if let Some(q) = scenarios::measure_render_query(&buffer, lang, mib) {
        eprintln!(
            "  query vp    p50={} p95={} p99={} (×{})",
            fmt_dur(q.p50()),
            fmt_dur(q.p95()),
            fmt_dur(q.p99()),
            q.count(),
        );
        percentile_rows.push(q);
    }
    if let Some(r) = scenarios::measure_incremental_reparse(&buffer, lang, mib) {
        eprintln!(
            "  inc reparse p50={} p95={} p99={} (×{})",
            fmt_dur(r.p50()),
            fmt_dur(r.p95()),
            fmt_dur(r.p99()),
            r.count(),
        );
        percentile_rows.push(r);
    }
}

fn load_buffer(path: &Path) -> Buffer {
    let file = std::fs::File::open(path).expect("必须能打开基准语料文件");
    let reader = std::io::BufReader::with_capacity(64 * 1024, file);
    let origin = BufferOrigin::external(path.to_string_lossy().into_owned());
    Buffer::from_reader(origin, reader, BufferConfig::default()).expect("基准语料必须能解码")
}

fn print_table(rows: &[Measurement]) {
    println!();
    println!(
        "{:<14} {:>5} {:>6} {:>14} {:>14}  备注",
        "场景", "MiB", "迭代", "总耗时", "单次耗时"
    );
    println!("{:-<90}", "");
    for r in rows {
        println!(
            "{:<14} {:>5} {:>6} {:>14} {:>14}  {}",
            r.scenario,
            r.size_mib,
            r.iters,
            fmt_dur(r.total),
            fmt_dur(r.per_iter()),
            r.note,
        );
    }
}

fn print_percentile_table(rows: &[PercentileMeasurement]) {
    if rows.is_empty() {
        return;
    }
    println!();
    println!(
        "{:<22} {:>5} {:>5} {:>12} {:>12} {:>12}  备注",
        "Phase 0 场景", "MiB", "样本", "p50", "p95", "p99"
    );
    println!("{:-<100}", "");
    for r in rows {
        println!(
            "{:<22} {:>5} {:>5} {:>12} {:>12} {:>12}  {}",
            r.scenario,
            r.size_mib,
            r.count(),
            fmt_dur(r.p50()),
            fmt_dur(r.p95()),
            fmt_dur(r.p99()),
            r.note,
        );
    }
}

fn fmt_dur(d: Duration) -> String {
    let nanos = d.as_nanos();
    if nanos < 10_000 {
        format!("{nanos} ns")
    } else if nanos < 10_000_000 {
        format!("{:.2} µs", nanos as f64 / 1_000.0)
    } else if nanos < 10_000_000_000 {
        format!("{:.2} ms", nanos as f64 / 1_000_000.0)
    } else {
        format!("{:.2} s", nanos as f64 / 1_000_000_000.0)
    }
}

fn print_usage() {
    println!("zom-bench —— 16 MiB 基线测量套件");
    println!();
    println!("用法:");
    println!("  zom-bench corpus               生成语料到 target/bench-corpus/");
    println!(
        "  zom-bench run <lang> [size]    跑测量。lang 为 rust|json|log|all；size 可选 1|4|16"
    );
    println!();
    println!("示例:");
    println!("  cargo run --release -p zom-bench -- corpus");
    println!("  cargo run --release -p zom-bench -- run rust");
    println!("  cargo run --release -p zom-bench -- run rust 16");
    println!("  cargo run --release -p zom-bench -- run all");
}
