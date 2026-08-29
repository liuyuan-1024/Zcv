//! 本地项目内容搜索。
//!
//! 磁盘遍历、文本匹配与未打开文件的加载全部在后台完成；
//! 命中文件随扫描进度逐文件通过通道流出，UI 线程按批装配进 MultiBuffer ordered excerpts。
//! 接收方放弃通道（新搜索取代或视图关闭）时，后台在下次发送时感知并提前结束扫描。

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;

use async_channel::{Receiver, Sender};
use gpui::Task;
use zcv_text::{Buffer, BufferConfig, ByteOffset, Line, SearchQuery, Snapshot, TextRange};

use crate::worktree::WorktreeSearchPlan;

const CONTEXT_LINES: usize = 2;
const MAX_MATCHES: usize = 10_000;

/// 后台搜索逐文件产出的命中；由 UI 线程逐批装配。
pub struct FileSearchResult {
    pub path: PathBuf,
    pub display_path: PathBuf,
    pub excerpts: Vec<ExcerptMatches>,
    // 未打开文件的预加载内容；命中已打开文件时为空（走 BufferStore 缓存）。
    pub loaded_buffer: Option<Buffer>,
}

/// 单个命中在源文件中的上下文块：整块范围与块内全部命中范围。
pub struct ExcerptMatches {
    pub range: TextRange,
    pub matches: Vec<TextRange>,
}

/// 项目搜索的流式结果：后台扫描任务 + 逐文件结果通道。
pub struct SearchResults {
    pub task: Task<()>,
    pub rx: Receiver<FileSearchResult>,
}

impl SearchResults {
    /// 构造立即关闭的空结果流（无 worktree 时使用）。
    pub fn empty() -> Self {
        let (tx, rx) = async_channel::bounded(1);
        drop(tx);
        Self {
            task: Task::ready(()),
            rx,
        }
    }
}

pub(crate) async fn search_worktree(
    plan: WorktreeSearchPlan,
    opened_snapshots: HashMap<PathBuf, Snapshot>,
    query: SearchQuery,
    tx: Sender<FileSearchResult>,
) -> anyhow::Result<()> {
    if query.query.is_empty() {
        return Ok(());
    }
    validate_query(&query)?;

    let mut paths = git_search_paths(&plan).unwrap_or_else(|| {
        let mut paths = Vec::new();
        collect_files(&plan.root, &plan, &mut paths);
        paths
    });
    paths.sort();

    let mut total_matches = 0;
    for path in paths {
        // 已打开文件用内存快照搜索；其余文件在后台读盘并保留 Buffer，
        // 避免结果装配阶段在主线程重新读文件。
        let (snapshot, loaded_buffer) = if let Some(snapshot) = opened_snapshots.get(&path) {
            (snapshot.clone(), None)
        } else {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(buffer) = Buffer::scratch(text, BufferConfig::default()) else {
                continue;
            };
            (buffer.snapshot(), Some(buffer))
        };
        let mut matches = search_snapshot(&snapshot, &query)?;
        if matches.is_empty() {
            continue;
        }
        let remaining = MAX_MATCHES.saturating_sub(total_matches);
        if matches.len() > remaining {
            matches.truncate(remaining);
        }
        total_matches += matches.len();
        let display_path = path.strip_prefix(&plan.root).unwrap_or(&path).to_path_buf();
        let result = FileSearchResult {
            path,
            display_path,
            excerpts: excerpt_matches(&snapshot, &matches),
            loaded_buffer,
        };
        // 接收方已放弃（新搜索取代或视图关闭）时提前结束扫描。
        if tx.send(result).await.is_err() {
            break;
        }
        if total_matches == MAX_MATCHES {
            break;
        }
    }
    Ok(())
}

fn validate_query(query: &SearchQuery) -> anyhow::Result<()> {
    let buffer = Buffer::scratch(String::new(), BufferConfig::default())?;
    let _ = search_snapshot(&buffer.snapshot(), query)?;
    Ok(())
}

fn search_snapshot(snapshot: &Snapshot, query: &SearchQuery) -> anyhow::Result<Vec<TextRange>> {
    Ok(query.search(snapshot)?.ranges().collect())
}

fn collect_files(dir: &Path, plan: &WorktreeSearchPlan, output: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if plan.is_excluded(&path) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_files(&path, plan, output);
        } else if file_type.is_file() {
            output.push(path);
        }
    }
}

/// Git worktree 优先使用 Git 自己的候选文件集：已跟踪 + 未跟踪但未忽略。
/// 避免进入 target/node_modules 等 `.gitignore` 已排除的巨大目录。
/// 非 Git 目录或 Git 不可用时回退到递归扫描。
fn git_search_paths(plan: &WorktreeSearchPlan) -> Option<Vec<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(&plan.root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    Some(
        output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(git_path_from_bytes)
            .map(|relative| plan.root.join(relative))
            .filter(|path| !plan.is_excluded(path) && path.is_file())
            .collect(),
    )
}

#[cfg(unix)]
fn git_path_from_bytes(path: &[u8]) -> PathBuf {
    PathBuf::from(OsString::from_vec(path.to_vec()))
}

#[cfg(not(unix))]
fn git_path_from_bytes(path: &[u8]) -> PathBuf {
    PathBuf::from(OsString::from(String::from_utf8_lossy(path).into_owned()))
}

fn excerpt_matches(snapshot: &Snapshot, matches: &[TextRange]) -> Vec<ExcerptMatches> {
    let mut excerpts: Vec<ExcerptMatches> = Vec::new();
    for &matched in matches {
        let start_line = snapshot.byte_to_line(matched.start()).unwrap_or(Line::ZERO);
        let match_end = if matched.is_empty() {
            matched.end()
        } else {
            ByteOffset::new(matched.end().get().saturating_sub(1))
        };
        let end_line = snapshot.byte_to_line(match_end).unwrap_or(start_line);
        let context_start = Line::new(start_line.get().saturating_sub(CONTEXT_LINES));
        let context_end = (end_line.get() + CONTEXT_LINES + 1).min(snapshot.line_count());
        let start = snapshot
            .line_start_byte(context_start)
            .unwrap_or(ByteOffset::ZERO);
        let end = if context_end == snapshot.line_count() {
            snapshot.len_bytes()
        } else {
            snapshot
                .line_start_byte(Line::new(context_end))
                .unwrap_or(snapshot.len_bytes())
        };
        let range = TextRange::new(start, end).expect("上下文范围必须正序");
        if let Some(previous) = excerpts.last_mut()
            && range.start() <= previous.range.end()
        {
            previous.range = TextRange::new(previous.range.start(), range.end())
                .expect("合并后的上下文范围必须正序");
            previous.matches.push(matched);
        } else {
            excerpts.push(ExcerptMatches {
                range,
                matches: vec![matched],
            });
        }
    }
    excerpts
}

#[cfg(test)]
#[path = "test/search_tests.rs"]
mod tests;
