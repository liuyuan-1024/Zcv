//! 本地 LaTeX 工具链调用与预览产物发布。

use std::{
    env,
    ffi::OsString,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context as _, Result};

pub(super) fn is_latex_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("tex"))
}

pub(super) fn compile(source: &Path) -> Result<PathBuf> {
    let parent = source.parent().context("源文件缺少所在目录")?;
    let file_name = source.file_name().context("源文件缺少名称")?;
    let directory = parent.join(".zcv-latex").join(file_name);
    fs::create_dir_all(&directory)
        .with_context(|| format!("创建编译目录失败：{}", directory.display()))?;
    let staging = tempfile::tempdir_in(&directory).context("创建临时编译目录失败")?;

    let mut command = Command::new("latexmk");
    configure_toolchain(&mut command)?;
    let mut outdir = OsString::from("-outdir=");
    outdir.push(staging.path());
    command
        .current_dir(parent)
        .args([
            "-norc",
            "-xelatex",
            "-xelatex=xelatex -no-shell-escape %O %S",
            "-interaction=nonstopmode",
            "-halt-on-error",
            "-file-line-error",
            "-jobname=preview",
        ])
        .arg(outdir)
        .arg(Path::new(".").join(file_name));
    let output = command.output().map_err(|error| {
        let message = if error.kind() == io::ErrorKind::NotFound {
            "找不到 latexmk，请安装含 latexmk 和 XeLaTeX 的 TeX 发行版，并确认工具在 PATH 中"
        } else {
            "启动 latexmk 失败"
        };
        anyhow::Error::new(error).context(message)
    })?;
    publish_preview(&directory, staging.path(), output)
}

fn configure_toolchain(command: &mut Command) -> Result<()> {
    let mut paths = env::split_paths(&env::var_os("PATH").unwrap_or_default()).collect::<Vec<_>>();
    extend_toolchain_path(&mut paths);
    command.env(
        "PATH",
        env::join_paths(paths).context("组装 TeX 工具搜索路径失败")?,
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn extend_toolchain_path(paths: &mut Vec<PathBuf>) {
    // 从 Finder 启动时，PATH 通常不包含 MacTeX 的工具目录。
    paths.push(PathBuf::from("/Library/TeX/texbin"));
}

#[cfg(not(target_os = "macos"))]
fn extend_toolchain_path(_: &mut Vec<PathBuf>) {}

#[derive(Debug)]
pub(super) struct CompilerError {
    summary: String,
    pub(super) output: String,
}

impl std::fmt::Display for CompilerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.summary)
    }
}

impl std::error::Error for CompilerError {}

fn publish_preview(directory: &Path, staging: &Path, output: Output) -> Result<PathBuf> {
    if !output.status.success() {
        let compiler_output = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let summary = if compiler_output.lines().any(|line| {
            line.contains("xelatex")
                && (line.contains("not found") || line.contains("not recognized"))
        }) {
            "找不到 XeLaTeX，请检查 TeX 发行版和 PATH".into()
        } else {
            format!("latexmk 编译出错（{}）", output.status)
        };
        return Err(CompilerError {
            summary,
            output: compiler_output,
        }
        .into());
    }

    let preview = directory.join("preview.pdf");
    let mut pdf = File::open(staging.join("preview.pdf")).context("编译结果缺少 PDF 文件")?;
    let mut pending = tempfile::NamedTempFile::new_in(directory).context("创建预览文件失败")?;
    io::copy(&mut pdf, &mut pending).context("写入预览文件失败")?;
    pending.persist(&preview).context("替换预览文件失败")?;
    Ok(preview)
}

#[cfg(test)]
#[path = "test/build_tests.rs"]
mod tests;
