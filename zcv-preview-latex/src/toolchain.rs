//! 一次预览构建所使用的本地 TeX 工具和进程错误语义。

mod platform;

use std::{
    env,
    ffi::{OsStr, OsString},
    fmt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context as _, Result};

pub(super) struct Toolchain {
    latexmk: PathBuf,
    xelatex: PathBuf,
    search_path: OsString,
}

impl Toolchain {
    pub(super) fn discover() -> Result<Self> {
        let path = env::var_os("PATH").unwrap_or_default();
        let directories = env::split_paths(&path)
            .filter(|directory| directory.is_absolute())
            .chain(platform::search_directories())
            .collect();
        Self::resolve(directories)
    }

    fn resolve(directories: Vec<PathBuf>) -> Result<Self> {
        let (directory, latexmk, xelatex) = directories
            .iter()
            .find_map(|directory| {
                Some((
                    directory,
                    platform::executable(directory, "latexmk")?,
                    platform::executable(directory, "xelatex")?,
                ))
            })
            .context("找不到同一工具目录下的 latexmk 和 XeLaTeX，请检查 TeX 发行版及其 PATH")?;
        let search_path = env::join_paths(
            std::iter::once(directory).chain(directories.iter().filter(|path| *path != directory)),
        )
        .context("组装 TeX 工具搜索路径失败")?;
        Ok(Self {
            latexmk,
            xelatex,
            search_path,
        })
    }

    pub(super) fn compile(
        &self,
        parent: &Path,
        file_name: &OsStr,
        staging: &Path,
    ) -> Result<Output> {
        let version = run(
            Command::new(&self.xelatex)
                .env("PATH", &self.search_path)
                .current_dir(parent)
                .arg("--version"),
            "读取 XeLaTeX 版本",
        )?;
        check_output(&version, "读取 XeLaTeX 版本失败")?;
        let version = String::from_utf8_lossy(&version.stdout);
        run(
            &mut self.latexmk_command(parent, file_name, staging, &version),
            "启动 latexmk",
        )
    }

    fn latexmk_command(
        &self,
        parent: &Path,
        file_name: &OsStr,
        staging: &Path,
        version: &str,
    ) -> Command {
        // MiKTeX 的 BibTeX 0.99e 可能在解析参考文献样式时崩溃，参见 MiKTeX/miktex#1769。
        let bibtex = if version.starts_with("MiKTeX-XeTeX ") {
            "miktex-bibtexu"
        } else {
            "bibtex"
        };
        let mut outdir = OsString::from("-outdir=");
        outdir.push(staging);
        let mut command = Command::new(&self.latexmk);
        command
            .env("PATH", &self.search_path)
            .current_dir(parent)
            .args([
                "-norc",
                "-xelatex",
                "-xelatex=xelatex -no-shell-escape %O %S",
                "-interaction=nonstopmode",
                "-halt-on-error",
                "-file-line-error",
                "-jobname=preview",
                "-e",
            ])
            .arg(format!("$bibtex = q{{{bibtex} %O %S}};"))
            .arg(outdir)
            .arg(Path::new(".").join(file_name));
        command
    }
}

fn run(command: &mut Command, operation: &str) -> Result<Output> {
    command.output().with_context(|| {
        format!(
            "{operation}失败（{}）",
            Path::new(command.get_program()).display()
        )
    })
}

pub(super) fn check_output(output: &Output, failure: &str) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let summary = if let Some(path) = miktex_log_access_denied(&stderr) {
        format!(
            "{failure}：MiKTeX 无法写入日志（{path}）。请检查日志目录权限；若 Zcv 从受限环境启动，请从普通桌面重新启动。"
        )
    } else {
        format!("{failure}（{}）", output.status)
    };
    Err(CompilerError {
        summary,
        output: format!("{stdout}\n{stderr}"),
    }
    .into())
}

fn miktex_log_access_denied(stderr: &str) -> Option<&str> {
    // log4cxx 使用 APR 编码报告 Windows 的 ERROR_ACCESS_DENIED。
    let mut lines = stderr.lines();
    while let Some(line) = lines.next() {
        if let Some(path) = line
            .strip_prefix("log4cxx: setFile(")
            .and_then(|line| line.strip_suffix(",true) call failed."))
            && lines.next().is_some_and(|line| {
                line.starts_with("log4cxx: IO Exception : status code = 720005(")
            })
        {
            return Some(path);
        }
    }
    None
}

#[derive(Debug)]
pub(super) struct CompilerError {
    summary: String,
    pub(super) output: String,
}

impl fmt::Display for CompilerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.summary)
    }
}

impl std::error::Error for CompilerError {}

#[cfg(test)]
#[path = "test/toolchain_tests.rs"]
mod tests;
