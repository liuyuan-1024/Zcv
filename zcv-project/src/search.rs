//! 本地项目内容搜索。
//!
//! 磁盘遍历和文本匹配在后台完成；
//! 命中文件回到 Project 的 BufferStore 打开，最终产出 MultiBuffer ordered excerpts。

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;

use gpui::App;
use zcv_multi_buffer::MultiBufferExcerpt;
use zcv_text::{Buffer, BufferConfig, ByteOffset, Line, SearchQuery, Snapshot, TextRange};

use crate::buffer_store::BufferStore;
use crate::worktree::WorktreeSearchPlan;

const CONTEXT_LINES: usize = 2;
const MAX_MATCHES: usize = 10_000;

pub struct ProjectSearchResults {
    excerpts: Vec<MultiBufferExcerpt>,
    pub match_count: usize,
    pub file_count: usize,
    pub limit_reached: bool,
}

impl ProjectSearchResults {
    pub fn empty() -> Self {
        Self {
            excerpts: Vec::new(),
            match_count: 0,
            file_count: 0,
            limit_reached: false,
        }
    }

    pub fn into_excerpts(self) -> Vec<MultiBufferExcerpt> {
        self.excerpts
    }
}

pub(crate) struct WorktreeMatches {
    files: Vec<FileMatches>,
    limit_reached: bool,
}

struct FileMatches {
    path: PathBuf,
    display_path: PathBuf,
    excerpts: Vec<ExcerptMatches>,
}

struct ExcerptMatches {
    range: TextRange,
    matches: Vec<TextRange>,
}

pub(crate) fn search_worktree(
    plan: WorktreeSearchPlan,
    opened_snapshots: HashMap<PathBuf, Snapshot>,
    query: SearchQuery,
) -> anyhow::Result<WorktreeMatches> {
    if query.query.is_empty() {
        return Ok(WorktreeMatches {
            files: Vec::new(),
            limit_reached: false,
        });
    }
    validate_query(&query)?;

    let mut paths = git_search_paths(&plan).unwrap_or_else(|| {
        let mut paths = Vec::new();
        collect_files(&plan.root, &plan, &mut paths);
        paths
    });
    paths.sort();

    let mut files = Vec::new();
    let mut total_matches = 0;
    let mut limit_reached = false;
    for path in paths {
        let snapshot = if let Some(snapshot) = opened_snapshots.get(&path) {
            snapshot.clone()
        } else {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(buffer) = Buffer::scratch(text, BufferConfig::default()) else {
                continue;
            };
            buffer.snapshot()
        };
        let mut matches = search_snapshot(&snapshot, &query)?;
        if matches.is_empty() {
            continue;
        }
        let remaining = MAX_MATCHES.saturating_sub(total_matches);
        if matches.len() > remaining {
            matches.truncate(remaining);
            limit_reached = true;
        }
        total_matches += matches.len();
        let display_path = path.strip_prefix(&plan.root).unwrap_or(&path).to_path_buf();
        files.push(FileMatches {
            path,
            display_path,
            excerpts: excerpt_matches(&snapshot, &matches),
        });
        if total_matches == MAX_MATCHES {
            limit_reached = true;
            break;
        }
    }
    Ok(WorktreeMatches {
        files,
        limit_reached,
    })
}

pub(crate) fn materialize_results(
    matches: WorktreeMatches,
    buffer_store: &mut BufferStore,
    cx: &mut App,
) -> anyhow::Result<ProjectSearchResults> {
    let mut excerpts = Vec::new();
    let mut match_count = 0;
    let mut file_count = 0;
    for file in matches.files {
        let Ok(source) = buffer_store.open_buffer(&file.path, cx) else {
            continue;
        };
        file_count += 1;
        for excerpt in file.excerpts {
            match_count += excerpt.matches.len();
            excerpts.push(
                MultiBufferExcerpt::new(source.clone(), excerpt.range, excerpt.matches)
                    .with_display_path(file.display_path.clone()),
            );
        }
    }
    Ok(ProjectSearchResults {
        excerpts,
        match_count,
        file_count,
        limit_reached: matches.limit_reached,
    })
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
/// 这与 Zed 的 worktree 搜索边界一致，避免进入 target/node_modules 等 `.gitignore` 已排除的巨大目录。
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
