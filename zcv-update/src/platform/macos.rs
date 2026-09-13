use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context as _, Result, ensure};
use semver::Version;
use smol::process::Command;

use super::UpdateInstallation;

pub const APP_DIRECTORY_NAME: &str = "Zcv.app";
pub const APP_EXECUTABLE_RELATIVE_PATH: &str = "Contents/MacOS/Zcv";
pub const HELPER_RELATIVE_PATH: &str = "Contents/Helpers/zcv-update-helper";

pub fn from_process_path(process_path: &Path) -> Result<UpdateInstallation> {
    ensure!(
        process_path.file_name().and_then(|name| name.to_str()) == Some(APP_DIRECTORY_NAME),
        "当前 app bundle 不是 Zcv.app"
    );
    ensure!(
        !is_translocated_path(process_path),
        "应用运行在 App Translocation 临时路径中，请把 Zcv.app 移到 /Applications 后重启"
    );
    Ok(UpdateInstallation {
        app_path: process_path.to_path_buf(),
        platform_key: platform_key()?,
    })
}

pub fn archive_entry_is_allowed(name: &str) -> bool {
    name == APP_DIRECTORY_NAME
        || name.starts_with(&format!("{APP_DIRECTORY_NAME}/"))
        || name == "__MACOSX"
        || name == "__MACOSX/"
        || name == "__MACOSX/._Zcv.app"
        || name.starts_with("__MACOSX/Zcv.app/")
}

pub fn validate_extracted_app(app: &Path) -> Result<()> {
    ensure!(
        app.join(APP_EXECUTABLE_RELATIVE_PATH).is_file(),
        "更新包缺少应用可执行文件"
    );
    ensure!(
        app.join(HELPER_RELATIVE_PATH).is_file(),
        "更新包缺少更新辅助程序"
    );
    ensure!(
        app.join("Contents/Info.plist").is_file(),
        "更新包缺少 Info.plist"
    );
    Ok(())
}

pub fn prepare_helper(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

pub fn replace_file(temporary: &Path, destination: &Path) -> Result<()> {
    fs::rename(temporary, destination)
        .with_context(|| format!("无法提交文件 {}", destination.display()))?;
    Ok(())
}

pub fn verify_app(app: &Path, expected_version: &Version) -> Result<()> {
    ensure!(
        app.join(APP_EXECUTABLE_RELATIVE_PATH).is_file(),
        "更新应用缺少可执行文件"
    );
    let verify = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict"])
        .arg(app)
        .output()
        .context("无法启动 codesign 验证更新")?;
    ensure!(
        verify.status.success(),
        "更新应用代码签名无效：{}",
        String::from_utf8_lossy(&verify.stderr).trim()
    );

    let version = std::process::Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print:CFBundleShortVersionString"])
        .arg(app.join("Contents/Info.plist"))
        .output()
        .context("无法读取更新应用版本")?;
    ensure!(version.status.success(), "无法读取更新应用版本");
    ensure!(
        String::from_utf8_lossy(&version.stdout).trim() == expected_version.to_string(),
        "更新应用版本与清单目标版本不一致"
    );
    Ok(())
}

pub async fn extract_archive(archive: &Path, destination: &Path, _bytes: Vec<u8>) -> Result<()> {
    let output = Command::new("/usr/bin/ditto")
        .args([OsStr::new("-x"), OsStr::new("-k")])
        .arg(archive)
        .arg(destination)
        .stdout(Stdio::null())
        .output()
        .await
        .context("无法启动 ditto 解压更新")?;
    ensure!(
        output.status.success(),
        "ditto 解压更新失败：{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

pub fn is_translocated_path(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "AppTranslocation")
}

fn platform_key() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "aarch64" => Ok("macos-aarch64"),
        architecture => anyhow::bail!("不支持 macOS 自动更新架构 {architecture}"),
    }
}
