//! git diff 输出解析：`git diff --unified=0` 的行级 hunks。

use zcv_buffer_diff::{DiffHunk, DiffHunkKind};

/// 解析单个文件的 `git diff --unified=0` 输出（忽略全部路径头，按 hunk header 建块）。
///
/// 容错风格对齐 `parse_numstat`：格式异常的行跳过而非报错。
/// 只依赖 header 中的起止与计数，body 行（含 `\ No newline at end of file`）不参与统计；
/// `Binary files ... differ` 与空输出均返回空。
pub fn parse_diff_hunks(output: &[u8]) -> Vec<DiffHunk> {
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

#[cfg(test)]
mod tests {
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
}
