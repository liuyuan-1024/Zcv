//! 搜索能力域：查询模型、单 Buffer 匹配/替换与本地项目内容搜索。
//!
//! 磁盘遍历与文本匹配在后台完成；未打开文件经唯一的文件解码入口读取只读文本视图用于匹配，
//! 不创建也不登记权威文档实体，权威文档始终由 Project 文件边界按路径打开并复用。
//! 命中文件随扫描进度逐文件通过通道流出，UI 线程按批装配进 MultiBuffer ordered excerpts。
//! 接收方放弃通道（新搜索取代或视图关闭）时，后台在下次发送时感知并提前结束扫描。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_channel::{Receiver, Sender};
use futures::{StreamExt, stream};
use gpui::{BackgroundExecutor, Task};
use zcv_git::GitRepository;
use zcv_language::LanguageRegistry;
use zcv_path::AbsolutePathBuf;
use zcv_text::{ByteOffset, Line, Snapshot, TextRange, WordBoundaryPolicy};

use crate::buffer_store::load_buffer;
use crate::worktree::{WorktreeSearchPlan, discover_git_repository};

mod buffer_search;
mod error;
mod versioned;

pub use buffer_search::{
    PreparedSearchQuery, RegexSearchResult, SearchQuery, SearchQueryResult, SearchResult,
    regex_replacement_for_match, regex_replacements_in_text,
};

const CONTEXT_LINES: usize = 2;
const MAX_MATCHES: usize = 10_000;

/// 后台搜索逐文件产出的命中；由 UI 线程逐批装配。
pub struct FileSearchResult {
    pub path: PathBuf,
    pub display_path: PathBuf,
    pub excerpts: Vec<ExcerptMatches>,
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
    pub(crate) fn empty() -> Self {
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
    opened_snapshots: HashMap<AbsolutePathBuf, Snapshot>,
    query: SearchQuery,
    language_registry: Arc<LanguageRegistry>,
    tx: Sender<FileSearchResult>,
    background_executor: BackgroundExecutor,
) -> anyhow::Result<()> {
    if query.query.is_empty() {
        return Ok(());
    }
    // 查询只解析一次；尤其是正则查询，编译出的自动机会复用于每个文件快照。
    let prepared_query = query.prepare()?;

    // `git ls-files` 已按路径排序；只有递归文件系统回退路径需要额外排序。
    let paths = if let Some(paths) = git_search_paths(&plan) {
        paths
    } else {
        let mut paths: Vec<AbsolutePathBuf> = Vec::new();
        collect_files(&plan.root, &plan, &mut paths);
        paths.sort();
        paths
    };
    if paths.is_empty() {
        return Ok(());
    }

    let worker_count = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(paths.len())
        .min(8);
    let opened_snapshots = Arc::new(opened_snapshots);
    let prepared_query = Arc::new(prepared_query);
    let language_registry = Arc::new(language_registry);
    let root = Arc::new(plan.root);

    // `buffered` 同时限制在途读取数与乱序完成结果的保留量。
    // 不能先让 worker 把所有完成项塞进按序重排表：当前序号较慢时，后续命中的整文件 Buffer 会持续累积。
    let mut results = stream::iter(paths)
        .map(|path| {
            let opened_snapshots = Arc::clone(&opened_snapshots);
            let prepared_query = Arc::clone(&prepared_query);
            let language_registry = Arc::clone(&language_registry);
            let root = Arc::clone(&root);
            let background_executor = background_executor.clone();
            async move {
                background_executor
                    .spawn(async move {
                        search_file(
                            path,
                            &root,
                            &opened_snapshots,
                            &prepared_query,
                            &language_registry,
                        )
                    })
                    .await
            }
        })
        .buffered(worker_count);

    let mut total_matches = 0;
    while let Some(result) = results.next().await {
        let Some(mut result) = result else {
            continue;
        };
        let remaining = MAX_MATCHES.saturating_sub(total_matches);
        if remaining == 0 {
            return Ok(());
        }
        if result
            .excerpts
            .iter()
            .map(|excerpt| excerpt.matches.len())
            .sum::<usize>()
            > remaining
        {
            truncate_excerpts(&mut result.excerpts, remaining);
        }
        total_matches += result
            .excerpts
            .iter()
            .map(|excerpt| excerpt.matches.len())
            .sum::<usize>();
        if tx.send(result).await.is_err() || total_matches == MAX_MATCHES {
            return Ok(());
        }
    }
    Ok(())
}

fn search_file(
    path: AbsolutePathBuf,
    root: &AbsolutePathBuf,
    opened_snapshots: &HashMap<AbsolutePathBuf, Snapshot>,
    query: &PreparedSearchQuery,
    language_registry: &Arc<LanguageRegistry>,
) -> Option<FileSearchResult> {
    // 已打开文件直接搜索其权威快照；
    // 其余文件经唯一的文件解码入口读取只读文本视图，不创建也不登记权威文档实体。
    let snapshot = if let Some(snapshot) = opened_snapshots.get(&path) {
        snapshot.clone()
    } else {
        load_buffer(path.as_path()).ok()?.snapshot()
    };
    let word_boundary = language_registry
        .language_for_file(path.as_path(), None)
        .map_or_else(WordBoundaryPolicy::default, |language| {
            language.word_boundary()
        });
    let matches = search_snapshot(&snapshot, query, word_boundary).ok()?;
    if matches.is_empty() {
        return None;
    }
    Some(FileSearchResult {
        display_path: path
            .as_path()
            .strip_prefix(root.as_path())
            .unwrap_or_else(|_| path.as_path())
            .to_path_buf(),
        path: path.into_path_buf(),
        excerpts: excerpt_matches(&snapshot, &matches),
    })
}

fn truncate_excerpts(excerpts: &mut Vec<ExcerptMatches>, limit: usize) {
    let mut remaining = limit;
    let mut keep = 0;
    for excerpt in excerpts.iter_mut() {
        if excerpt.matches.len() > remaining {
            excerpt.matches.truncate(remaining);
        }
        remaining = remaining.saturating_sub(excerpt.matches.len());
        keep += 1;
        if remaining == 0 {
            break;
        }
    }
    excerpts.truncate(keep);
}

fn search_snapshot(
    snapshot: &Snapshot,
    query: &PreparedSearchQuery,
    word_boundary: WordBoundaryPolicy,
) -> anyhow::Result<Vec<TextRange>> {
    Ok(query.search(snapshot, word_boundary)?.ranges().collect())
}

fn collect_files(
    dir: &AbsolutePathBuf,
    plan: &WorktreeSearchPlan,
    output: &mut Vec<AbsolutePathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir.as_path()) else {
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
            let Ok(path) = AbsolutePathBuf::new(path) else {
                continue;
            };
            collect_files(&path, plan, output);
        } else if file_type.is_file()
            && let Ok(path) = AbsolutePathBuf::new(path)
        {
            output.push(path);
        }
    }
}

/// Git worktree 优先使用 Git 自己的候选文件集：已跟踪 + 未跟踪但未忽略。
/// 避免进入 target/node_modules 等 `.gitignore` 已排除的巨大目录。
/// 非 Git 目录或 Git 不可用时回退到递归扫描。
fn git_search_paths(plan: &WorktreeSearchPlan) -> Option<Vec<AbsolutePathBuf>> {
    let repository = discover_git_repository(plan.root.as_path()).ok()??;
    let working_directory =
        AbsolutePathBuf::new(repository.working_directory().to_path_buf()).ok()?;
    Some(
        repository
            .list_worktree_files()
            .ok()?
            .into_iter()
            .filter_map(|relative| {
                let path = AbsolutePathBuf::new(working_directory.as_path().join(relative)).ok()?;
                // 项目根可能位于外层仓库内：Git 会列出根之外的文件，这里按项目根收敛搜索范围。
                path.as_path().strip_prefix(plan.root.as_path()).ok()?;
                // Git 输出的是文件条目；去掉逐文件 metadata 查询，实际读取失败时仍由下方文件解码路径自然跳过。
                (!plan.is_excluded(path.as_path())).then_some(path)
            })
            .collect(),
    )
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
#[path = "../test/search_tests.rs"]
mod tests;
