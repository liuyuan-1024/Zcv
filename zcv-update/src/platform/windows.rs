use std::path::Path;

use anyhow::{Context as _, Result, ensure};
use async_zip::base::read::mem::ZipFileReader;
use futures::io::AsyncWriteExt as _;
use semver::Version;

use super::UpdateInstallation;

pub const APP_DIRECTORY_NAME: &str = "Zcv";
pub const APP_EXECUTABLE_RELATIVE_PATH: &str = "Zcv.exe";
pub const HELPER_RELATIVE_PATH: &str = "zcv-update-helper.exe";

pub fn from_process_path(process_path: &Path) -> Result<UpdateInstallation> {
    ensure!(
        process_path.file_name().and_then(|name| name.to_str()) == Some("Zcv.exe"),
        "当前进程不是 Zcv.exe"
    );
    let app_path = process_path
        .parent()
        .context("Zcv.exe 没有安装目录")?
        .to_path_buf();
    ensure!(
        app_path.file_name().and_then(|name| name.to_str()) == Some(APP_DIRECTORY_NAME),
        "当前安装目录不是 Zcv"
    );
    Ok(UpdateInstallation {
        app_path,
        platform_key: platform_key()?,
    })
}

pub fn archive_entry_is_allowed(name: &str) -> bool {
    name == APP_DIRECTORY_NAME || name.starts_with(&format!("{APP_DIRECTORY_NAME}/"))
}

pub fn validate_extracted_app(app: &Path) -> Result<()> {
    ensure!(
        app.join(APP_EXECUTABLE_RELATIVE_PATH).is_file(),
        "更新包缺少 Zcv.exe"
    );
    ensure!(
        app.join(HELPER_RELATIVE_PATH).is_file(),
        "更新包缺少更新辅助程序"
    );
    ensure!(app.join("version.txt").is_file(), "更新包缺少 version.txt");
    Ok(())
}

pub fn prepare_helper(_: &Path) -> Result<()> {
    Ok(())
}

pub fn replace_file(temporary: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        std::fs::remove_file(destination)
            .with_context(|| format!("无法替换旧文件 {}", destination.display()))?;
    }
    std::fs::rename(temporary, destination)
        .with_context(|| format!("无法提交文件 {}", destination.display()))?;
    Ok(())
}

pub fn verify_app(app: &Path, expected_version: &Version) -> Result<()> {
    validate_extracted_app(app)?;
    let version =
        std::fs::read_to_string(app.join("version.txt")).context("无法读取更新应用版本")?;
    ensure!(
        version.trim() == expected_version.to_string(),
        "更新应用版本与清单目标版本不一致"
    );
    Ok(())
}

pub async fn extract_archive(_archive: &Path, destination: &Path, bytes: Vec<u8>) -> Result<()> {
    let reader = ZipFileReader::new(bytes).await?;
    for index in 0..reader.file().entries().len() {
        let entry = &reader.file().entries()[index];
        let name = entry
            .filename()
            .as_str()
            .context("更新压缩包包含非 UTF-8 路径")?;
        let path = destination.join(name);
        if entry.dir().context("无法读取更新压缩包目录项")? {
            smol::fs::create_dir_all(&path).await?;
            continue;
        }
        let parent = path.parent().context("更新压缩包文件没有父目录")?;
        smol::fs::create_dir_all(parent).await?;
        let mut entry_reader = reader.reader_without_entry(index).await?;
        let mut output = smol::fs::File::create(&path).await?;
        futures::io::copy(&mut entry_reader, &mut output).await?;
        output.flush().await?;
    }
    Ok(())
}

fn platform_key() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("windows-x86_64"),
        "aarch64" => Ok("windows-aarch64"),
        architecture => anyhow::bail!("不支持 Windows 自动更新架构 {architecture}"),
    }
}
