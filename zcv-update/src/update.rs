//! Zcv 自动更新的可信清单、产物校验与跨进程事务协议。
//!
//! 应用进程负责下载并验证更新；`zcv-update-helper` 只消费已经验证的事务，
//! 在应用退出后完成原子切换。
//! 该 crate 不持有 GPUI 状态，也不决定检查策略。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(target_os = "macos")]
use std::ffi::OsStr;
#[cfg(target_os = "macos")]
use std::process::Stdio;

use anyhow::{Context as _, Result, ensure};
use async_zip::base::read::mem::ZipFileReader;
use base64::Engine as _;
use ring::signature;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
#[cfg(target_os = "macos")]
use smol::process::Command;

pub const MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const TRANSACTION_SCHEMA_VERSION: u32 = 2;
pub const STABLE_CHANNEL: &str = "stable";
pub const APP_BUNDLE_NAME: &str = "Zcv.app";
pub const APP_EXECUTABLE_RELATIVE_PATH: &str = "Contents/MacOS/Zcv";
pub const HELPER_RELATIVE_PATH: &str = "Contents/Helpers/zcv-update-helper";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub channel: String,
    pub version: Version,
    pub published_at: String,
    pub assets: BTreeMap<String, ReleaseAsset>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedRelease {
    pub version: Version,
    pub asset: ReleaseAsset,
}

impl ReleaseManifest {
    pub fn select_newer_release(
        &self,
        current_version: &Version,
        platform: &str,
    ) -> Result<Option<SelectedRelease>> {
        ensure!(
            self.schema_version == MANIFEST_SCHEMA_VERSION,
            "不支持更新清单 schema_version {}",
            self.schema_version
        );
        ensure!(self.channel == STABLE_CHANNEL, "更新清单不是 stable 通道");
        ensure!(!self.published_at.trim().is_empty(), "更新清单缺少发布时间");
        if self.version <= *current_version {
            return Ok(None);
        }

        let asset = self
            .assets
            .get(platform)
            .with_context(|| format!("更新清单缺少平台产物 {platform}"))?
            .clone();
        validate_asset(&asset)?;
        Ok(Some(SelectedRelease {
            version: self.version.clone(),
            asset,
        }))
    }
}

fn validate_asset(asset: &ReleaseAsset) -> Result<()> {
    ensure!(
        asset.url.starts_with("https://"),
        "更新产物必须使用 HTTPS 地址"
    );
    ensure!(asset.size > 0, "更新产物大小必须大于零");
    ensure!(asset.sha256.len() == 64, "更新产物 SHA-256 长度无效");
    ensure!(
        asset
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
        "更新产物 SHA-256 必须是小写十六进制"
    );
    Ok(())
}

pub fn verify_and_parse_manifest(
    manifest_bytes: &[u8],
    signature_base64: &[u8],
    public_key_base64: &str,
) -> Result<ReleaseManifest> {
    let public_key = base64::engine::general_purpose::STANDARD
        .decode(public_key_base64.trim())
        .context("更新公钥不是合法 Base64")?;
    ensure!(public_key.len() == 32, "更新公钥长度无效");
    let signature = base64::engine::general_purpose::STANDARD
        .decode(
            std::str::from_utf8(signature_base64)
                .context("更新清单签名不是 UTF-8")?
                .trim(),
        )
        .context("更新清单签名不是合法 Base64")?;
    signature::UnparsedPublicKey::new(&signature::ED25519, public_key)
        .verify(manifest_bytes, &signature)
        .map_err(|_| anyhow::anyhow!("更新清单签名验证失败"))?;
    serde_json::from_slice(manifest_bytes).context("更新清单不是合法 JSON")
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        fs::File::open(path).with_context(|| format!("无法打开更新产物 {}", path.display()))?;
    let mut digest = Sha256::new();
    std::io::copy(&mut file, &mut digest)
        .with_context(|| format!("无法读取更新产物 {}", path.display()))?;
    Ok(format!("{:x}", digest.finalize()))
}

pub fn verify_downloaded_asset(path: &Path, asset: &ReleaseAsset) -> Result<()> {
    let metadata =
        fs::metadata(path).with_context(|| format!("无法读取更新产物元数据 {}", path.display()))?;
    ensure!(
        metadata.len() == asset.size,
        "更新产物大小不匹配：预期 {} 字节，实际 {} 字节",
        asset.size,
        metadata.len()
    );
    let actual = sha256_file(path)?;
    ensure!(actual == asset.sha256, "更新产物 SHA-256 不匹配");
    Ok(())
}

/// 验证更新包中的 app bundle。
///
/// 零费用方案下 bundle 使用 ad-hoc 签名，没有 Developer ID 身份，也无法通过公证，
/// 因此这里只验证代码签名有效性和版本一致性，不检查 Team ID 与 Gatekeeper 评估。
pub fn verify_macos_app(app: &Path, expected_version: &Version) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        ensure!(
            app.join(APP_EXECUTABLE_RELATIVE_PATH).is_file(),
            "更新应用缺少可执行文件"
        );
        // 不传 --deep：seal 本身覆盖 bundle 内全部文件，嵌套签名由主签名完整性保证。
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
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, expected_version);
        anyhow::bail!("当前平台尚不支持应用自动更新")
    }
}

pub async fn extract_verified_archive(archive: &Path, destination: &Path) -> Result<PathBuf> {
    let bytes = smol::fs::read(archive)
        .await
        .with_context(|| format!("无法读取更新压缩包 {}", archive.display()))?;
    validate_archive(&bytes).await?;

    if smol::fs::metadata(destination).await.is_ok() {
        smol::fs::remove_dir_all(destination)
            .await
            .with_context(|| format!("无法清理旧暂存目录 {}", destination.display()))?;
    }
    smol::fs::create_dir_all(destination)
        .await
        .with_context(|| format!("无法创建暂存目录 {}", destination.display()))?;

    #[cfg(target_os = "macos")]
    {
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
    }
    #[cfg(not(target_os = "macos"))]
    anyhow::bail!("当前平台尚不支持应用自动更新");

    let app_path = destination.join(APP_BUNDLE_NAME);
    ensure!(
        app_path.join(APP_EXECUTABLE_RELATIVE_PATH).is_file(),
        "更新包缺少 Zcv 可执行文件"
    );
    ensure!(
        app_path.join(HELPER_RELATIVE_PATH).is_file(),
        "更新包缺少 zcv-update-helper"
    );
    ensure!(
        app_path.join("Contents/Info.plist").is_file(),
        "更新包缺少 Info.plist"
    );
    Ok(app_path)
}

async fn validate_archive(bytes: &[u8]) -> Result<()> {
    let reader = ZipFileReader::new(bytes.to_vec())
        .await
        .context("更新产物不是合法 ZIP")?;
    ensure!(!reader.file().entries().is_empty(), "更新压缩包为空");
    let mut has_app = false;
    let mut total_uncompressed = 0_u64;
    for stored in reader.file().entries() {
        let entry = stored;
        let name = entry
            .filename()
            .as_str()
            .context("更新压缩包包含非 UTF-8 路径")?;
        validate_archive_entry_path(name)?;
        has_app |= name == APP_BUNDLE_NAME || name.starts_with("Zcv.app/");
        total_uncompressed = total_uncompressed
            .checked_add(entry.uncompressed_size())
            .context("更新压缩包展开大小溢出")?;
        ensure!(
            total_uncompressed <= 2 * 1024 * 1024 * 1024,
            "更新压缩包展开后超过 2 GiB"
        );
        if let Some(mode) = entry.unix_permissions() {
            ensure!(mode & 0o170000 != 0o120000, "更新压缩包不允许符号链接");
        }
    }
    ensure!(has_app, "更新压缩包缺少 Zcv.app");
    Ok(())
}

fn validate_archive_entry_path(name: &str) -> Result<()> {
    ensure!(!name.contains('\\'), "更新压缩包路径包含反斜杠");
    ensure!(!name.contains('\0'), "更新压缩包路径包含空字节");
    let path = Path::new(name);
    ensure!(!path.is_absolute(), "更新压缩包包含绝对路径");
    ensure!(
        path.components()
            .all(|component| matches!(component, Component::Normal(_))),
        "更新压缩包包含不安全路径 {name}"
    );
    let allowed = name == APP_BUNDLE_NAME
        || name.starts_with("Zcv.app/")
        || name == "__MACOSX"
        || name == "__MACOSX/"
        || name == "__MACOSX/._Zcv.app"
        || name.starts_with("__MACOSX/Zcv.app/");
    ensure!(allowed, "更新压缩包包含意外顶层路径 {name}");
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct UpdateTransaction {
    pub schema_version: u32,
    pub id: String,
    pub from_version: Version,
    pub to_version: Version,
    pub install_path: PathBuf,
    pub staged_app_path: PathBuf,
    pub result_path: PathBuf,
}

impl UpdateTransaction {
    pub fn new(
        from_version: Version,
        to_version: Version,
        install_path: PathBuf,
        staged_app_path: PathBuf,
        result_path: PathBuf,
    ) -> Result<Self> {
        ensure!(to_version > from_version, "更新目标版本必须高于当前版本");
        ensure!(
            install_path.file_name() == Some(OsStr::new(APP_BUNDLE_NAME)),
            "安装路径必须指向 Zcv.app"
        );
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("系统时间早于 UNIX_EPOCH")?
            .as_nanos();
        Ok(Self {
            schema_version: TRANSACTION_SCHEMA_VERSION,
            id: format!("{}-{timestamp}", std::process::id()),
            from_version,
            to_version,
            install_path,
            staged_app_path,
            result_path,
        })
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == TRANSACTION_SCHEMA_VERSION,
            "不支持更新事务 schema_version {}",
            self.schema_version
        );
        ensure!(self.to_version > self.from_version, "更新事务版本顺序无效");
        ensure!(
            self.install_path.file_name() == Some(OsStr::new(APP_BUNDLE_NAME)),
            "更新事务安装路径无效"
        );
        ensure!(
            self.staged_app_path.file_name() == Some(OsStr::new(APP_BUNDLE_NAME)),
            "更新事务暂存路径无效"
        );
        Ok(())
    }
}

/// 判断路径是否位于 App Translocation 转译目录。
///
/// 从浏览器下载的 ad-hoc 应用直接双击运行时会被 macOS 转译到只读随机路径；
/// 此时安装路径不是用户看到的位置，自更新会写入转译路径并在重启后丢失，
/// 因此转译状态下必须禁用自动更新。
pub fn is_translocated_path(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == "AppTranslocation")
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateResultStatus {
    Applied,
    RolledBack,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct UpdateResult {
    pub transaction_id: String,
    pub from_version: Version,
    pub to_version: Version,
    pub status: UpdateResultStatus,
    pub error: Option<String>,
}

pub fn atomic_write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("JSON 路径没有父目录")?;
    fs::create_dir_all(parent).with_context(|| format!("无法创建目录 {}", parent.display()))?;
    let temporary = path.with_extension("json.tmp");
    let content = serde_json::to_vec_pretty(value).context("无法序列化 JSON")?;
    {
        let mut file = fs::File::create(&temporary)
            .with_context(|| format!("无法创建临时文件 {}", temporary.display()))?;
        std::io::Write::write_all(&mut file, &content)
            .with_context(|| format!("无法写入临时文件 {}", temporary.display()))?;
        file.sync_all()
            .with_context(|| format!("无法同步临时文件 {}", temporary.display()))?;
    }
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("无法替换旧文件 {}", path.display()))?;
    }
    fs::rename(&temporary, path).with_context(|| format!("无法提交文件 {}", path.display()))?;
    Ok(())
}

pub fn read_transaction(path: &Path) -> Result<UpdateTransaction> {
    let transaction: UpdateTransaction = serde_json::from_slice(
        &fs::read(path).with_context(|| format!("无法读取更新事务 {}", path.display()))?,
    )
    .context("更新事务不是合法 JSON")?;
    transaction.validate()?;
    Ok(transaction)
}

#[cfg(test)]
#[path = "test/update_tests.rs"]
mod tests;
