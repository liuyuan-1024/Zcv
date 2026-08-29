//! git diff 输出解析：`git diff --unified=0` 的行级 hunks。

use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;

use crate::status::path_from_bytes;

/// 单块行级 diff：新侧逻辑行范围（0-based，左闭右开）+ 旧侧范围 + 变化类型。
///
/// - Added/Modified：range 为新增行的行号区间（纯增时旧侧计数为 0）；
/// - Deleted：range 为空区间，锚定 newStart−1 行（删除发生处的行），渲染侧展开为一个显示行。
/// - `old_range` 是旧侧（HEAD 版本）的行范围：Deleted 时用它从 HEAD 文本切片出被删除的行。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DiffHunk {
    pub range: Range<usize>,
    /// 旧侧（HEAD）行范围：Deleted 为被删除行；Added 为 oldStart..oldStart；Modified 两侧同行。
    pub old_range: Range<usize>,
    pub kind: DiffHunkKind,
}

/// hunk 变化类型（判定规则对齐 Zed buffer_diff：旧侧空→Added、新侧空→Deleted）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiffHunkKind {
    /// 旧侧计数为 0（纯新增）。
    Added,
    /// 新旧两侧计数均非 0。
    Modified,
    /// 新侧计数为 0（纯删除）。
    Deleted,
}

/// 解析单个文件的 `git diff --unified=0` 输出（忽略全部路径头，按 hunk header 建块）。
///
/// 容错风格对齐 `parse_numstat`：格式异常的行跳过而非报错。
/// 只依赖 header 中的起止与计数，body 行（含 `\ No newline at end of file`）不参与统计；
/// `Binary files ... differ` 与空输出均返回空。
///
/// 生产路径使用多段解析的 [`parse_diff_hunks_per_path`]（带路径核对），本函数仅测试用。
#[cfg(test)]
pub(crate) fn parse_diff_hunks(output: &[u8]) -> Vec<DiffHunk> {
    let mut hunks = Vec::new();
    for line in output.split(|byte| *byte == b'\n') {
        if line.starts_with(b"@@ -") {
            if let Some(hunk) = parse_hunk_header(line) {
                hunks.push(hunk);
            }
        } else if line.starts_with(b"Binary files ") {
            return Vec::new();
        }
    }
    hunks
}

/// 解析 `@@ -oldStart[,oldCount] +newStart[,newCount] @@` 头；畸形返回 None（跳过）。
fn parse_hunk_header(line: &[u8]) -> Option<DiffHunk> {
    let mut tokens = line
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|token| !token.is_empty());
    if tokens.next()? != b"@@".as_slice() {
        return None;
    }
    let old = tokens.next()?.strip_prefix(b"-")?;
    let new = tokens.next()?.strip_prefix(b"+")?;
    if tokens.next()? != b"@@".as_slice() {
        return None;
    }
    let (old_start, old_count) = parse_range_part(old)?;
    let (new_start, new_count) = parse_range_part(new)?;
    let old_count = old_count.unwrap_or(1);
    let new_count = new_count.unwrap_or(1);

    let kind = if old_count == 0 {
        DiffHunkKind::Added
    } else if new_count == 0 {
        DiffHunkKind::Deleted
    } else {
        DiffHunkKind::Modified
    };
    let old_start = old_start.saturating_sub(1) as usize;
    let old_range = old_start..old_start + old_count as usize;
    let new_start = new_start.saturating_sub(1) as usize;
    let range = if new_count == 0 {
        new_start..new_start
    } else {
        new_start..new_start + new_count as usize
    };
    Some(DiffHunk {
        range,
        old_range,
        kind,
    })
}

/// 解析 `start[,count]`；count 缺省返回 None（git 对 count==1 省略 `,1`）。
fn parse_range_part(token: &[u8]) -> Option<(u64, Option<u64>)> {
    let mut parts = token.split(|byte| *byte == b',');
    let start = std::str::from_utf8(parts.next()?).ok()?.parse().ok()?;
    let count = if let Some(part) = parts.next() {
        Some(std::str::from_utf8(part).ok()?.parse().ok()?)
    } else {
        None
    };
    if parts.next().is_some() {
        return None;
    }
    Some((start, count))
}

/// 按 `diff --git` 头切分多文件 diff 输出（配合 `core.quotepath=false`），返回 路径 → hunks。
///
/// `requested` 是命令传入的路径集合，用于核对解析出的路径；解析不出或不在集合中的段跳过，
/// 该文件的 hunks 缺失但整批不中断（缺失项由下次 status 刷新后的 hunks 补齐自愈）。
pub(crate) fn parse_diff_hunks_per_path(
    output: &[u8],
    requested: &[PathBuf],
) -> Vec<(PathBuf, Vec<DiffHunk>)> {
    let mut sections: Vec<(Option<PathBuf>, Vec<DiffHunk>)> = Vec::new();
    for line in output.split(|byte| *byte == b'\n') {
        if let Some(rest) = line.strip_prefix(b"diff --git ") {
            sections.push((parse_diff_git_path(rest), Vec::new()));
            continue;
        }
        let Some((_, hunks)) = sections.last_mut() else {
            continue;
        };
        if line.starts_with(b"@@ -") {
            if let Some(hunk) = parse_hunk_header(line) {
                hunks.push(hunk);
            }
        } else if line.starts_with(b"Binary files ") {
            // 二进制段无可显示的行差异（对齐 parse_diff_hunks 语义）。
            hunks.clear();
        }
    }
    let requested: HashSet<&std::path::Path> = requested.iter().map(PathBuf::as_path).collect();
    sections
        .into_iter()
        .filter_map(|(path, hunks)| match path {
            Some(path) if requested.contains(path.as_path()) => Some((path, hunks)),
            _ => None,
        })
        .collect()
}

/// 从单文件 `git diff --unified=0` 输出中提取一个可独立应用的 hunk patch。
///
/// 文件头只保留一份，目标 hunk 之外的 body 不进入结果；
/// 调用方可把结果交给 `git apply --unidiff-zero`，用于 hunk 级暂存、取消暂存或还原。
pub(crate) fn select_hunk_patch(output: &[u8], target: &DiffHunk) -> Option<Vec<u8>> {
    let mut first_hunk_start = None;
    let mut target_range: Option<Range<usize>> = None;
    let mut offset = 0usize;

    for line in output.split_inclusive(|byte| *byte == b'\n') {
        if line.starts_with(b"@@ -") {
            first_hunk_start.get_or_insert(offset);
            if let Some(range) = &mut target_range {
                range.end = offset;
                break;
            }
            let header = line.strip_suffix(b"\n").unwrap_or(line);
            if parse_hunk_header(header).as_ref() == Some(target) {
                target_range = Some(offset..output.len());
            }
        }
        offset += line.len();
    }

    let first_hunk_start = first_hunk_start?;
    let target_range = target_range?;
    let mut patch = Vec::with_capacity(first_hunk_start + target_range.len());
    patch.extend_from_slice(&output[..first_hunk_start]);
    patch.extend_from_slice(&output[target_range]);
    Some(patch)
}

/// 返回目标 hunk 在旧侧和新侧的真实行编辑范围。
///
/// `DiffHunk` 的空侧范围为显示而锚在前一行；index 编辑则必须使用
/// unified diff header 中的边界值，否则纯新增和纯删除会产生一行偏差。
pub(crate) fn select_hunk_edit_ranges(
    output: &[u8],
    target: &DiffHunk,
) -> Option<(Range<usize>, Range<usize>)> {
    output
        .split(|byte| *byte == b'\n')
        .filter(|line| line.starts_with(b"@@ -"))
        .find_map(|line| {
            (parse_hunk_header(line).as_ref() == Some(target))
                .then(|| parse_hunk_edit_ranges(line))
                .flatten()
        })
}

fn parse_hunk_edit_ranges(line: &[u8]) -> Option<(Range<usize>, Range<usize>)> {
    let mut tokens = line
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|token| !token.is_empty());
    if tokens.next()? != b"@@".as_slice() {
        return None;
    }
    let old = tokens.next()?.strip_prefix(b"-")?;
    let new = tokens.next()?.strip_prefix(b"+")?;
    if tokens.next()? != b"@@".as_slice() {
        return None;
    }
    let (old_start, old_count) = parse_range_part(old)?;
    let (new_start, new_count) = parse_range_part(new)?;
    Some((
        edit_range(old_start, old_count.unwrap_or(1))?,
        edit_range(new_start, new_count.unwrap_or(1))?,
    ))
}

fn edit_range(start: u64, count: u64) -> Option<Range<usize>> {
    let start = usize::try_from(start).ok()?;
    let count = usize::try_from(count).ok()?;
    let start = if count == 0 {
        start
    } else {
        start.saturating_sub(1)
    };
    Some(start..start.checked_add(count)?)
}

/// 解析 `diff --git a/X b/Y` 头中的 b 侧路径（当前工作树路径）。
///
/// 含 `"` 的路径是 C 引用格式（quotepath=false 下仅控制字符路径仍会引用），无法可靠解码，返回 None。
fn parse_diff_git_path(rest: &[u8]) -> Option<PathBuf> {
    if rest.contains(&b'"') {
        return None;
    }
    // 取最后一个 ` b/` 之后的部分（a、b 路径相同时 b 侧即最后一个路径段）。
    let index = rest.windows(3).rposition(|window| window == b" b/")?;
    Some(path_from_bytes(&rest[index + 3..]))
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::ffi::OsStr;
    #[cfg(unix)]
    use std::os::unix::ffi::OsStrExt;

    use super::*;
    use DiffHunkKind::*;

    fn parse(output: &str) -> Vec<DiffHunk> {
        parse_diff_hunks(output.as_bytes())
    }

    #[test]
    fn parses_pure_addition_hunk() {
        let output = "@@ -12,0 +13,3 @@\n+fn new_fn() {}\n+    let x = 1;\n+    let y = 2;\n";
        assert_eq!(
            parse(output),
            vec![DiffHunk {
                range: 12..15,
                old_range: 11..11,
                kind: Added,
            }]
        );
    }

    #[test]
    fn parses_pure_deletion_hunk() {
        let output = "@@ -30,2 +33,0 @@ fn removed\n-    let removed = 1;\n";
        assert_eq!(
            parse(output),
            vec![DiffHunk {
                range: 32..32,
                old_range: 29..31,
                kind: Deleted,
            }]
        );
    }

    #[test]
    fn isolated_hunk_edits_keep_true_empty_side_boundaries() {
        let added = b"@@ -0,0 +1,2 @@\n+first\n+second\n";
        let added_hunk = parse("@@ -0,0 +1,2 @@\n+first\n+second\n")
            .pop()
            .expect("应解析新增 hunk");
        assert_eq!(
            select_hunk_edit_ranges(added, &added_hunk),
            Some((0..0, 0..2))
        );

        let deleted = b"@@ -1,2 +0,0 @@\n-first\n-second\n";
        let deleted_hunk = parse("@@ -1,2 +0,0 @@\n-first\n-second\n")
            .pop()
            .expect("应解析删除 hunk");
        assert_eq!(
            select_hunk_edit_ranges(deleted, &deleted_hunk),
            Some((0..2, 0..0))
        );
    }

    #[test]
    fn parses_mixed_hunk_as_modified() {
        let output = "@@ -5,3 +5,3 @@\n-    let a = 1;\n+    let b = 2;\n     let c = 3;\n";
        assert_eq!(
            parse(output),
            vec![DiffHunk {
                range: 4..7,
                old_range: 4..7,
                kind: Modified,
            }]
        );
    }

    #[test]
    fn count_one_is_omitted() {
        let output = "@@ -3 +4 @@\n-old\n+new\n";
        assert_eq!(
            parse(output),
            vec![DiffHunk {
                range: 3..4,
                old_range: 2..3,
                kind: Modified,
            }]
        );
    }

    #[test]
    fn parses_multiple_hunks_in_order() {
        let output = "@@ -1,0 +1,2 @@\n+a\n+b\n@@ -10,2 +13,0 @@\n-x\n-y\n";
        assert_eq!(
            parse(output),
            vec![
                DiffHunk {
                    range: 0..2,
                    old_range: 0..0,
                    kind: Added,
                },
                DiffHunk {
                    range: 12..12,
                    old_range: 9..11,
                    kind: Deleted,
                },
            ]
        );
    }

    #[test]
    fn skips_no_newline_marker() {
        let output = "@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n";
        assert_eq!(
            parse(output),
            vec![DiffHunk {
                range: 0..1,
                old_range: 0..1,
                kind: Modified,
            }]
        );
    }

    #[test]
    fn binary_files_produce_no_hunks() {
        let output = "Binary files a/img.png and b/img.png differ\n";
        assert_eq!(parse(output), vec![]);
    }

    #[test]
    fn empty_output_produces_no_hunks() {
        assert_eq!(parse(""), vec![]);
    }

    #[test]
    fn ignores_function_context_suffix() {
        let output = "@@ -30,2 +33,0 @@ fn unchanged_context()\n-    let removed = 1;\n";
        assert_eq!(
            parse(output),
            vec![DiffHunk {
                range: 32..32,
                old_range: 29..31,
                kind: Deleted,
            }]
        );
    }

    #[test]
    fn new_start_one_maps_to_row_zero() {
        let output = "@@ -0,0 +1,5 @@\n+all\n+lines\n+are\n+new\n+here\n";
        assert_eq!(
            parse(output),
            vec![DiffHunk {
                range: 0..5,
                old_range: 0..0,
                kind: Added,
            }]
        );
    }

    #[test]
    fn skips_malformed_hunk_header() {
        let output = "@@ -abc +1 @@\n+new\n@@ -5,3 +5,3 @@\n";
        assert_eq!(
            parse(output),
            vec![DiffHunk {
                range: 4..7,
                old_range: 4..7,
                kind: Modified,
            }]
        );
    }

    #[test]
    fn ignores_path_and_metadata_lines() {
        let output = "diff --git a/main.rs b/main.rs\n\
            index 9d6f7c2..1a2b3c4 100644\n\
            --- a/main.rs\n\
            +++ b/main.rs\n\
            similarity index 100%\n\
            @@ -1 +1 @@\n\
            -old\n\
            +new\n";
        assert_eq!(
            parse(output),
            vec![DiffHunk {
                range: 0..1,
                old_range: 0..1,
                kind: Modified,
            }]
        );
    }

    #[test]
    fn parses_multiple_files_sections_in_order() {
        let output = "\
diff --git a/a.rs b/a.rs
index 111..222 100644
--- a/a.rs
+++ b/a.rs
@@ -1 +1 @@
-old
+new
diff --git a/b.rs b/b.rs
index 333..444 100644
--- a/b.rs
+++ b/b.rs
@@ -2,0 +3,2 @@
+one
+two
";
        let requested = vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")];
        let parsed = parse_diff_hunks_per_path(output.as_bytes(), &requested);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].0, PathBuf::from("a.rs"));
        assert_eq!(parsed[1].0, PathBuf::from("b.rs"));
        assert_eq!(parsed[1].1.len(), 1);
    }

    #[test]
    fn parses_paths_with_spaces_and_unicode() {
        let output = "\
diff --git a/带 空格.txt b/带 空格.txt
index 111..222 100644
--- a/带 空格.txt
+++ b/带 空格.txt
@@ -1 +1 @@
-old
+new
";
        let requested = vec![PathBuf::from("带 空格.txt")];
        let parsed = parse_diff_hunks_per_path(output.as_bytes(), &requested);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, PathBuf::from("带 空格.txt"));
        assert_eq!(parsed[0].1.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn parses_non_utf8_paths_as_bytes() {
        let path = b"src/\xff\xfe.rs";
        let mut output = b"diff --git a/".to_vec();
        output.extend_from_slice(path);
        output.extend_from_slice(b" b/");
        output.extend_from_slice(path);
        output.extend_from_slice(b"\n@@ -1 +1 @@\n-old\n+new\n");
        let requested = vec![PathBuf::from(OsStr::from_bytes(path))];
        let parsed = parse_diff_hunks_per_path(&output, &requested);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, PathBuf::from(OsStr::from_bytes(path)));
    }

    #[test]
    fn renamed_section_uses_b_side_path() {
        // 重命名段的 a/b 路径不同：归属取 b 侧（当前工作树路径）。
        let output = "\
diff --git a/old.rs b/new.rs
similarity index 90%
rename from old.rs
rename to new.rs
index 111..222 100644
--- a/old.rs
+++ b/new.rs
@@ -1 +1 @@
-old
+new
";
        let requested = vec![PathBuf::from("new.rs")];
        let parsed = parse_diff_hunks_per_path(output.as_bytes(), &requested);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, PathBuf::from("new.rs"));
        assert_eq!(parsed[0].1.len(), 1);
    }

    #[test]
    fn binary_section_produces_no_hunks() {
        let output = "\
diff --git a/img.png b/img.png
index 111..222 100644
Binary files a/img.png and b/img.png differ
";
        let requested = vec![PathBuf::from("img.png")];
        let parsed = parse_diff_hunks_per_path(output.as_bytes(), &requested);
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].1.is_empty());
    }

    #[test]
    fn quoted_path_section_is_skipped() {
        // 控制字符路径即使 quotepath=false 也总是 C 引用，无法可靠解码 → 跳过该段。
        let output = "\
diff --git \"a/\\t\\303\\251.txt\" \"b/\\t\\303\\251.txt\"
index 111..222 100644
--- \"a/\\t\\303\\251.txt\"
+++ \"b/\\t\\303\\251.txt\"
@@ -1 +1 @@
-old
+new
";
        let requested = vec![PathBuf::from("x.txt")];
        let parsed = parse_diff_hunks_per_path(output.as_bytes(), &requested);
        assert!(parsed.is_empty());
    }

    #[test]
    fn section_not_in_requested_set_is_skipped() {
        let output = "\
diff --git a/other.rs b/other.rs
index 111..222 100644
--- a/other.rs
+++ b/other.rs
@@ -1 +1 @@
-old
+new
";
        let requested = vec![PathBuf::from("a.rs")];
        let parsed = parse_diff_hunks_per_path(output.as_bytes(), &requested);
        assert!(parsed.is_empty());
    }

    #[test]
    fn selects_one_hunk_as_an_independent_patch() {
        let output = b"diff --git a/a.txt b/a.txt\nindex 123..456 100644\n--- a/a.txt\n+++ b/a.txt\n@@ -2 +2 @@\n-old two\n+new two\n@@ -8 +8 @@\n-old eight\n+new eight\n";
        let target = DiffHunk {
            range: 7..8,
            old_range: 7..8,
            kind: Modified,
        };

        assert_eq!(
            select_hunk_patch(output, &target).unwrap(),
            b"diff --git a/a.txt b/a.txt\nindex 123..456 100644\n--- a/a.txt\n+++ b/a.txt\n@@ -8 +8 @@\n-old eight\n+new eight\n"
        );
    }
}
