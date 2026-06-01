//! 合成语料生成：rust / json / log 三种文本，落盘到 `target/bench-corpus/`。
//!
//! 文本只追求「形态像真实代码」。
//! tree-sitter 解析路径会经过关键字、字符串、标识符和嵌套等典型分支。
//! 这里不追求语义正确，也不追求与真实仓库等价。

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::Lang;

const SIZES_MIB: &[usize] = &[1, 4, 16, 64];

pub fn corpus_dir() -> PathBuf {
    let manifest = env!("CARGO_MANIFEST_DIR");
    PathBuf::from(manifest)
        .parent()
        .expect("CARGO_MANIFEST_DIR 必须有父目录")
        .join("target")
        .join("bench-corpus")
}

pub fn fixture_path(lang: Lang, size_mib: usize) -> PathBuf {
    corpus_dir().join(format!(
        "{}-{}mb.{}",
        lang.name(),
        size_mib,
        lang.extension()
    ))
}

pub fn sizes() -> &'static [usize] {
    SIZES_MIB
}

pub fn ensure_all() -> io::Result<()> {
    let dir = corpus_dir();
    fs::create_dir_all(&dir)?;
    for &lang in &[Lang::Rust, Lang::Json, Lang::Log] {
        for &mib in SIZES_MIB {
            let path = fixture_path(lang, mib);
            if path.exists() {
                let actual = fs::metadata(&path)?.len() as usize;
                let target = mib * 1024 * 1024;
                if actual >= target && actual < target + 4096 {
                    continue;
                }
            }
            write_fixture(&path, lang, mib * 1024 * 1024)?;
        }
    }
    Ok(())
}

fn write_fixture(path: &Path, lang: Lang, target_bytes: usize) -> io::Result<()> {
    let mut file = fs::File::create(path)?;
    let mut writer = io::BufWriter::with_capacity(1 << 20, &mut file);
    let mut written = 0usize;
    let mut counter: u64 = 0;
    let mut scratch = String::with_capacity(4096);
    while written < target_bytes {
        scratch.clear();
        match lang {
            Lang::Rust => emit_rust_chunk(&mut scratch, &mut counter),
            Lang::Json => emit_json_chunk(&mut scratch, &mut counter, written == 0),
            Lang::Log => emit_log_chunk(&mut scratch, &mut counter),
        }
        let remaining = target_bytes - written;
        let bytes = scratch.as_bytes();
        let take = bytes.len().min(remaining);
        writer.write_all(&bytes[..take])?;
        written += take;
    }
    if matches!(lang, Lang::Json) {
        // JSON 顶层是数组，结尾补上 "\n]\n"。
        // 语料用于性能测量，不用于合法性校验。
        let _ = writer.write_all(b"\n]\n");
    }
    writer.flush()?;
    Ok(())
}

fn emit_rust_chunk(out: &mut String, counter: &mut u64) {
    use std::fmt::Write;
    for _ in 0..16 {
        let n = *counter;
        *counter += 1;
        let _ = writeln!(
            out,
            r#"/// generated function #{n} — synthetic body to exercise tree-sitter.
pub fn item_{n}(input: &str, depth: usize) -> Result<String, std::io::Error> {{
    let mut acc = String::from("prefix-{n}");
    for (idx, ch) in input.chars().enumerate() {{
        if idx % 3 == 0 && depth > 0 {{
            acc.push(ch);
        }} else if let Some(next) = input.get(idx..idx + 1) {{
            acc.push_str(next);
        }}
    }}
    Ok(acc)
}}
"#
        );
    }
}

fn emit_json_chunk(out: &mut String, counter: &mut u64, first: bool) {
    use std::fmt::Write;
    if first {
        out.push_str("[\n");
    }
    for _ in 0..32 {
        let n = *counter;
        *counter += 1;
        let lead = if n == 0 { "" } else { ",\n" };
        let _ = write!(
            out,
            r#"{lead}  {{"id": {n}, "name": "item-{n}", "tags": ["alpha", "beta"], "nested": {{"k": "v-{n}", "v": {n}}}, "blob": "{n}-padding-padding-padding"}}"#
        );
    }
}

fn emit_log_chunk(out: &mut String, counter: &mut u64) {
    use std::fmt::Write;
    for _ in 0..32 {
        let n = *counter;
        *counter += 1;
        let level = ["INFO", "DEBUG", "WARN", "ERROR"][(n % 4) as usize];
        let _ = writeln!(
            out,
            "2026-06-01T12:{:02}:{:02}.{:03}Z {level} module::component[{n}]: event payload value={n} status=ok detail=\"sample log line emitted by synthetic corpus generator\"",
            (n / 60) % 60,
            n % 60,
            n % 1000
        );
    }
}
