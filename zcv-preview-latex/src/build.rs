//! LaTeX 预览的临时目录和产物发布。

use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    process::Output,
};

use anyhow::{Context as _, Result};

use super::toolchain::{self, Toolchain};

pub(super) fn is_latex_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("tex"))
}

pub(super) fn compile(source: &Path) -> Result<PathBuf> {
    let parent = source.parent().context("源文件缺少所在目录")?;
    let file_name = source.file_name().context("源文件缺少名称")?;
    let toolchain = Toolchain::discover()?;
    let directory = parent.join(".zcv-latex").join(file_name);
    fs::create_dir_all(&directory)
        .with_context(|| format!("创建编译目录失败：{}", directory.display()))?;
    let staging = tempfile::tempdir_in(&directory).context("创建临时编译目录失败")?;

    let output = toolchain.compile(parent, file_name, staging.path())?;
    publish_preview(&directory, staging.path(), output)
}

fn publish_preview(directory: &Path, staging: &Path, output: Output) -> Result<PathBuf> {
    toolchain::check_output(&output, "latexmk 编译失败")?;

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
