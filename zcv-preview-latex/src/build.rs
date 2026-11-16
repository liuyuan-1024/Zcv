//! LaTeX 预览的构建入口和 PDF 发布。

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result};

use super::toolchain::Toolchain;

const CACHE_DIRECTORY: &str = ".zcv-latex";

pub(super) fn is_latex_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("tex"))
}

pub(super) fn compile(source: &Path) -> Result<PathBuf> {
    let parent = source.parent().context("源文件缺少所在目录")?;
    let file_name = source.file_name().context("源文件缺少名称")?;
    let toolchain = Toolchain::discover()?;
    let relative_cache = Path::new(CACHE_DIRECTORY);
    let mut compiled = toolchain.compile(parent, file_name, relative_cache)?;
    let directory = parent.join(relative_cache).join(file_name);
    publish_preview(&directory, compiled.pdf())
}

fn publish_preview(directory: &Path, mut pdf: impl io::Read) -> Result<PathBuf> {
    let preview = directory.join("preview.pdf");
    fs::create_dir_all(directory)
        .with_context(|| format!("创建预览目录失败：{}", directory.display()))?;
    let mut pending = tempfile::NamedTempFile::new_in(directory).context("创建预览文件失败")?;
    io::copy(&mut pdf, &mut pending).context("写入预览文件失败")?;
    pending.persist(&preview).context("替换预览文件失败")?;
    Ok(preview)
}

#[cfg(test)]
#[path = "test/build_tests.rs"]
mod tests;
