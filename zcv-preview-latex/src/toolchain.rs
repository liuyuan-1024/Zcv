//! 本地 TeX 工具的选择、编译和临时产物。

mod platform;

use std::{
    env,
    ffi::{OsStr, OsString},
    fmt,
    fs::{self, File},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context as _, Result};

pub(super) struct CompiledPdf {
    // 先关闭 PDF，再清理编译目录。
    pdf: File,
    _directory: tempfile::TempDir,
}

impl CompiledPdf {
    pub(super) fn pdf(&mut self) -> &mut File {
        &mut self.pdf
    }
}

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
        relative_scratch_root: &Path,
        pdf_name: &Path,
    ) -> Result<CompiledPdf> {
        let version = run(
            Command::new(&self.xelatex)
                .env("PATH", &self.search_path)
                .current_dir(parent)
                .arg("--version"),
            "读取 XeLaTeX 版本",
        )?;
        check_output(&version, "读取 XeLaTeX 版本失败")?;
        let version = String::from_utf8_lossy(&version.stdout);

        let scratch_root = parent.join(relative_scratch_root);
        fs::create_dir_all(&scratch_root)
            .with_context(|| format!("创建编译目录失败：{}", scratch_root.display()))?;
        let directory = tempfile::tempdir_in(&scratch_root).context("创建临时编译目录失败")?;
        // TeX Live 在 Windows 上可能无法把含中文的输出目录写入 TEXMF_OUTPUT_DIRECTORY。
        let relative_staging = directory
            .path()
            .strip_prefix(parent)
            .context("临时编译目录不在源文件所在目录下")?;
        let output = run(
            &mut self.latexmk_command(parent, file_name, relative_staging, &version),
            "启动 latexmk",
        )?;
        check_output(&output, "latexmk 编译失败")?;
        let pdf_path = directory.path().join(pdf_name);
        let pdf = File::open(&pdf_path)
            .with_context(|| format!("读取编译生成的 PDF 失败：{}", pdf_path.display()))?;
        Ok(CompiledPdf {
            pdf,
            _directory: directory,
        })
    }

    fn latexmk_command(
        &self,
        parent: &Path,
        file_name: &OsStr,
        relative_staging: &Path,
        version: &str,
    ) -> Command {
        // MiKTeX 的 BibTeX 0.99e 可能在解析参考文献样式时崩溃，参见 MiKTeX/miktex#1769。
        let bibtex = if version.starts_with("MiKTeX-XeTeX ") {
            "miktex-bibtexu"
        } else {
            "bibtex"
        };
        let mut outdir = OsString::from("-outdir=");
        outdir.push(relative_staging);
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
            ])
            .arg("-e")
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

fn check_output(output: &Output, failure: &str) -> Result<()> {
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
