//! 应用级自动更新：启动时单次检查、可信下载、TopBar 状态与重启交接。

mod network_client;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result, ensure};
use gpui::http_client::{AsyncBody, HttpClient};
use gpui::{
    App, AppContext as _, BackgroundExecutor, Context, Entity, Global, Render, Subscription, Task,
    WeakEntity, Window, div, prelude::*,
};
use semver::Version;
use sha2::{Digest as _, Sha256};
use smol::io::{AsyncReadExt as _, AsyncWriteExt as _};
use zcv_actions::RestartToUpdate;
use zcv_ui::Button;
use zcv_update::{
    APP_BUNDLE_NAME, HELPER_RELATIVE_PATH, ReleaseAsset, SelectedRelease, UpdateTransaction,
    atomic_write_json, extract_verified_archive, is_translocated_path, verify_and_parse_manifest,
    verify_downloaded_asset, verify_macos_app,
};
use zcv_workspace::{ToastKind, Workspace};

const MANIFEST_URL: &str =
    "https://github.com/liuyuan-1024/Zcv/releases/latest/download/latest.json";
const UPDATE_PUBLIC_KEY: &str = "btPdamnS0mdny0zsaJA3qM/Du/XGAGXKQq93GC81Q0k=";
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_SIGNATURE_BYTES: usize = 16 * 1024;

pub(crate) fn new_http_client() -> Result<Arc<dyn HttpClient>> {
    network_client::new()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UpdateStatus {
    Disabled,
    Idle,
    Checking,
    Downloading {
        version: Version,
        progress_percent: u8,
    },
    ReadyToRestart {
        version: Version,
    },
    Failed {
        message: Arc<str>,
    },
}

#[derive(Clone)]
struct UpdateConfig {
    current_version: Version,
    manifest_url: String,
    public_key: &'static str,
    app_path: PathBuf,
    updates_dir: PathBuf,
    platform: &'static str,
}

#[derive(Clone)]
struct PreparedUpdate {
    version: Version,
    staged_app_path: PathBuf,
}

pub(crate) struct UpdateManager {
    status: UpdateStatus,
    config: Option<UpdateConfig>,
    prepared: Option<PreparedUpdate>,
    check_task: Option<Task<()>>,
    restart_started: bool,
}

#[derive(Default)]
struct GlobalUpdateManager(Option<Entity<UpdateManager>>);

impl Global for GlobalUpdateManager {}

pub(crate) fn init(cx: &mut App) {
    let config = UpdateConfig::from_app(cx).map_err(|error| {
        eprintln!("自动更新未启用：{error:#}");
        error
    });
    let manager = cx.new(|cx| UpdateManager::new(config.ok(), cx));
    cx.set_global(GlobalUpdateManager(Some(manager.clone())));
    manager.update(cx, |manager, cx| manager.check_for_update(cx));
}

pub(crate) fn acknowledge_started_update() -> Result<()> {
    let Some(ack_path) = std::env::var_os("ZCV_UPDATE_ACK_PATH").map(PathBuf::from) else {
        return Ok(());
    };
    let transaction_id =
        std::env::var("ZCV_UPDATE_TRANSACTION_ID").context("缺少 ZCV_UPDATE_TRANSACTION_ID")?;
    let updates_dir = zcv_settings::config_dir().join("updates");
    let expected_ack_name = format!("ack-{transaction_id}.json");
    ensure!(
        ack_path.parent() == Some(updates_dir.as_path())
            && ack_path.file_name().and_then(|name| name.to_str())
                == Some(expected_ack_name.as_str()),
        "更新确认路径与事务不匹配"
    );
    fs::create_dir_all(&updates_dir)
        .with_context(|| format!("无法创建更新确认目录 {}", updates_dir.display()))?;
    let temporary = ack_path.with_extension("json.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec(&serde_json::json!({ "transaction_id": transaction_id }))?,
    )
    .with_context(|| format!("无法写入更新启动确认 {}", temporary.display()))?;
    fs::rename(&temporary, &ack_path)
        .with_context(|| format!("无法提交更新启动确认 {}", ack_path.display()))?;
    Ok(())
}

impl UpdateConfig {
    fn from_app(cx: &App) -> Result<Self> {
        ensure!(cfg!(target_os = "macos"), "当前仅支持 macOS 自动更新");
        let app_path = cx.app_path().context("当前进程不是 macOS app bundle")?;
        ensure!(
            app_path.file_name().and_then(|name| name.to_str()) == Some(APP_BUNDLE_NAME),
            "当前 app bundle 不是 Zcv.app"
        );
        ensure!(
            !is_translocated_path(&app_path),
            "应用运行在 App Translocation 临时路径中，请把 Zcv.app 移到 /Applications 后重启"
        );
        let current_version = env!("CARGO_PKG_VERSION")
            .parse()
            .context("应用版本不是合法 SemVer")?;
        Ok(Self {
            current_version,
            manifest_url: MANIFEST_URL.to_owned(),
            public_key: UPDATE_PUBLIC_KEY,
            app_path,
            updates_dir: zcv_settings::config_dir().join("updates"),
            platform: platform_key()?,
        })
    }
}

fn platform_key() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "aarch64" => Ok("macos-aarch64"),
        architecture => anyhow::bail!("仅支持 Apple Silicon 自动更新架构 {architecture}"),
    }
}

impl UpdateManager {
    fn new(config: Option<UpdateConfig>, _cx: &mut Context<Self>) -> Self {
        Self {
            status: if config.is_some() {
                UpdateStatus::Idle
            } else {
                UpdateStatus::Disabled
            },
            config,
            prepared: None,
            check_task: None,
            restart_started: false,
        }
    }

    pub(crate) fn get(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalUpdateManager>()?.0.clone()
    }

    pub(crate) fn status(&self) -> UpdateStatus {
        self.status.clone()
    }

    fn check_for_update(&mut self, cx: &mut Context<Self>) {
        if self.check_task.is_some() || self.prepared.is_some() {
            return;
        }
        let Some(config) = self.config.clone() else {
            return;
        };
        self.status = UpdateStatus::Checking;
        cx.notify();

        let client = cx.http_client();
        let executor = cx.background_executor().clone();
        self.check_task = Some(cx.spawn(async move |this, cx| {
            let progress_entity = this.clone();
            let mut progress_cx = cx.clone();
            let result = check_and_stage(config, client, executor, move |version, percent| {
                progress_entity
                    .update(&mut progress_cx, |manager, cx| {
                        manager.status = UpdateStatus::Downloading {
                            version,
                            progress_percent: percent,
                        };
                        cx.notify();
                    })
                    .ok();
            })
            .await;

            this.update(cx, |manager, cx| {
                manager.check_task.take();
                match result {
                    Ok(CheckOutcome::Current) => manager.status = UpdateStatus::Idle,
                    Ok(CheckOutcome::Prepared(prepared)) => {
                        manager.status = UpdateStatus::ReadyToRestart {
                            version: prepared.version.clone(),
                        };
                        manager.prepared = Some(prepared);
                    }
                    Err(CheckFailure::Transient(error)) => {
                        eprintln!("自动检查更新暂时失败：{error:#}");
                        manager.status = UpdateStatus::Idle;
                    }
                    Err(CheckFailure::Permanent(error)) => {
                        eprintln!("自动更新产物无效：{error:#}");
                        manager.status = UpdateStatus::Failed {
                            message: Arc::from(error.to_string()),
                        };
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn launch_helper(&mut self) -> Result<()> {
        ensure!(!self.restart_started, "更新重启已经开始");
        let result = self.launch_helper_inner();
        if result.is_ok() {
            self.restart_started = true;
        }
        result
    }

    fn launch_helper_inner(&self) -> Result<()> {
        let config = self.config.as_ref().context("自动更新未启用")?;
        let prepared = self.prepared.as_ref().context("没有已暂存的更新")?;
        let result_path = config.updates_dir.join("last-result.json");
        let transaction = UpdateTransaction::new(
            config.current_version.clone(),
            prepared.version.clone(),
            config.app_path.clone(),
            prepared.staged_app_path.clone(),
            result_path,
        )?;
        let transaction_dir = config
            .updates_dir
            .join("transactions")
            .join(&transaction.id);
        fs::create_dir_all(&transaction_dir)
            .with_context(|| format!("无法创建更新事务目录 {}", transaction_dir.display()))?;
        let pending_path = config
            .updates_dir
            .join(format!("pending-{}.json", transaction.id));
        atomic_write_json(&pending_path, &transaction)?;

        let helper_source = config.app_path.join(HELPER_RELATIVE_PATH);
        let helper_path = transaction_dir.join("zcv-update-helper");
        fs::copy(&helper_source, &helper_path).with_context(|| {
            format!(
                "无法复制更新辅助程序 {} → {}",
                helper_source.display(),
                helper_path.display()
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let mut permissions = fs::metadata(&helper_path)?.permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&helper_path, permissions)?;
        }

        let log_path = transaction_dir.join("helper.log");
        let log = fs::File::create(&log_path)
            .with_context(|| format!("无法创建更新日志 {}", log_path.display()))?;
        Command::new(&helper_path)
            .arg("--transaction")
            .arg(&pending_path)
            .arg("--parent-pid")
            .arg(std::process::id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()
            .with_context(|| format!("无法启动更新辅助程序 {}", helper_path.display()))?;
        Ok(())
    }
}

enum CheckOutcome {
    Current,
    Prepared(PreparedUpdate),
}

enum CheckFailure {
    Transient(anyhow::Error),
    Permanent(anyhow::Error),
}

async fn check_and_stage(
    config: UpdateConfig,
    client: Arc<dyn HttpClient>,
    executor: BackgroundExecutor,
    mut on_progress: impl FnMut(Version, u8),
) -> std::result::Result<CheckOutcome, CheckFailure> {
    let manifest = fetch_limited(
        &client,
        &config.manifest_url,
        MAX_MANIFEST_BYTES,
        "更新清单",
    )
    .await?;
    let signature_url = format!("{}.sig", config.manifest_url);
    let signature =
        fetch_limited(&client, &signature_url, MAX_SIGNATURE_BYTES, "更新清单签名").await?;
    let manifest = verify_and_parse_manifest(&manifest, &signature, config.public_key)
        .map_err(CheckFailure::Permanent)?;
    let Some(release) = manifest
        .select_newer_release(&config.current_version, config.platform)
        .map_err(CheckFailure::Permanent)?
    else {
        return Ok(CheckOutcome::Current);
    };

    let release_dir = config.updates_dir.join(release.version.to_string());
    let archive_path = release_dir.join("Zcv.zip");
    download_release(&client, &release, &archive_path, &mut on_progress).await?;
    let staged_dir = release_dir.join("staged");
    let staged_app_path = extract_verified_archive(&archive_path, &staged_dir)
        .await
        .map_err(CheckFailure::Permanent)?;

    let app = staged_app_path.clone();
    let version = release.version.clone();
    executor
        .spawn(async move { verify_macos_app(&app, &version) })
        .await
        .map_err(CheckFailure::Permanent)?;

    Ok(CheckOutcome::Prepared(PreparedUpdate {
        version: release.version,
        staged_app_path,
    }))
}

async fn fetch_limited(
    client: &Arc<dyn HttpClient>,
    url: &str,
    limit: usize,
    label: &str,
) -> std::result::Result<Vec<u8>, CheckFailure> {
    let mut response = client
        .get(url, AsyncBody::empty(), true)
        .await
        .map_err(CheckFailure::Transient)?;
    if !response.status().is_success() {
        return Err(CheckFailure::Transient(anyhow::anyhow!(
            "获取{label}失败：HTTP {}",
            response.status()
        )));
    }
    let mut body = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = response
            .body_mut()
            .read(&mut buffer)
            .await
            .map_err(|error| CheckFailure::Transient(error.into()))?;
        if read == 0 {
            break;
        }
        if body.len() + read > limit {
            return Err(CheckFailure::Permanent(anyhow::anyhow!(
                "{label}超过大小上限"
            )));
        }
        body.extend_from_slice(&buffer[..read]);
    }
    Ok(body)
}

async fn download_release(
    client: &Arc<dyn HttpClient>,
    release: &SelectedRelease,
    archive_path: &Path,
    on_progress: &mut impl FnMut(Version, u8),
) -> std::result::Result<(), CheckFailure> {
    if archive_path.is_file() && verify_downloaded_asset(archive_path, &release.asset).is_ok() {
        on_progress(release.version.clone(), 100);
        return Ok(());
    }
    if archive_path.exists() {
        fs::remove_file(archive_path).map_err(|error| CheckFailure::Permanent(error.into()))?;
    }
    let parent = archive_path.parent().expect("更新压缩包路径应有父目录");
    smol::fs::create_dir_all(parent)
        .await
        .map_err(|error| CheckFailure::Permanent(error.into()))?;
    let partial = archive_path.with_extension("zip.part");
    let mut file = smol::fs::File::create(&partial)
        .await
        .map_err(|error| CheckFailure::Permanent(error.into()))?;
    let mut response = client
        .get(&release.asset.url, AsyncBody::empty(), true)
        .await
        .map_err(CheckFailure::Transient)?;
    if !response.status().is_success() {
        return Err(CheckFailure::Transient(anyhow::anyhow!(
            "下载更新失败：HTTP {}",
            response.status()
        )));
    }

    let mut digest = Sha256::new();
    let mut downloaded = 0_u64;
    let mut last_percent = None;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = response
            .body_mut()
            .read(&mut buffer)
            .await
            .map_err(|error| CheckFailure::Transient(error.into()))?;
        if read == 0 {
            break;
        }
        downloaded += read as u64;
        if downloaded > release.asset.size {
            return Err(CheckFailure::Permanent(anyhow::anyhow!(
                "更新下载超过清单声明大小"
            )));
        }
        digest.update(&buffer[..read]);
        file.write_all(&buffer[..read])
            .await
            .map_err(|error| CheckFailure::Permanent(error.into()))?;
        let percent = ((downloaded * 100) / release.asset.size).min(100) as u8;
        if last_percent != Some(percent) {
            last_percent = Some(percent);
            on_progress(release.version.clone(), percent);
        }
    }
    file.flush()
        .await
        .map_err(|error| CheckFailure::Permanent(error.into()))?;
    ensure_download_matches(downloaded, &digest.finalize(), &release.asset)
        .map_err(CheckFailure::Permanent)?;
    smol::fs::rename(&partial, archive_path)
        .await
        .map_err(|error| CheckFailure::Permanent(error.into()))?;
    on_progress(release.version.clone(), 100);
    Ok(())
}

fn ensure_download_matches(downloaded: u64, digest: &[u8], asset: &ReleaseAsset) -> Result<()> {
    ensure!(
        downloaded == asset.size,
        "更新下载大小不匹配：预期 {} 字节，实际 {} 字节",
        asset.size,
        downloaded
    );
    let actual = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    ensure!(actual == asset.sha256, "更新下载 SHA-256 不匹配");
    Ok(())
}

pub(crate) struct UpdateButton {
    status: UpdateStatus,
    _subscription: Option<Subscription>,
}

impl UpdateButton {
    pub(crate) fn new(workspace: WeakEntity<Workspace>, cx: &mut Context<Self>) -> Self {
        let Some(manager) = UpdateManager::get(cx) else {
            return Self {
                status: UpdateStatus::Disabled,
                _subscription: None,
            };
        };
        let status = manager.read(cx).status();
        if let Some(message) = new_failure(&UpdateStatus::Disabled, &status) {
            show_failure_toast(&workspace, message, cx);
        }
        let subscription = cx.observe(&manager, move |button, manager, cx| {
            let status = manager.read(cx).status();
            if let Some(message) = new_failure(&button.status, &status) {
                show_failure_toast(&workspace, message, cx);
            }
            button.status = status;
            cx.notify();
        });
        Self {
            status,
            _subscription: Some(subscription),
        }
    }
}

fn new_failure(previous: &UpdateStatus, current: &UpdateStatus) -> Option<Arc<str>> {
    match current {
        UpdateStatus::Failed { message } if previous != current => Some(message.clone()),
        _ => None,
    }
}

fn show_failure_toast(workspace: &WeakEntity<Workspace>, message: Arc<str>, cx: &mut App) {
    let workspace = workspace.clone();
    cx.defer(move |cx| {
        workspace
            .update(cx, |workspace, cx| {
                workspace.show_toast(
                    ToastKind::Error,
                    format!("自动更新失败：{message}"),
                    None,
                    Some(Duration::from_secs(8)),
                    cx,
                );
            })
            .ok();
    });
}

impl Render for UpdateButton {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        match &self.status {
            UpdateStatus::Downloading {
                version,
                progress_percent,
            } => Button::icon_text(
                "top-bar.update-downloading",
                "icons/download.svg",
                format!("{progress_percent}%"),
            )
            .label(format!("正在下载 Zcv {version}"))
            .disabled(true)
            .into_any_element(),
            UpdateStatus::ReadyToRestart { version } => Button::icon_text(
                "top-bar.update-ready",
                "icons/refresh_title.svg",
                "重启以更新",
            )
            .label(format!("Zcv {version} 已就绪"))
            .on_click(|_, window, cx| {
                window.dispatch_action(Box::new(RestartToUpdate), cx);
            })
            .into_any_element(),
            UpdateStatus::Disabled
            | UpdateStatus::Idle
            | UpdateStatus::Checking
            | UpdateStatus::Failed { .. } => div().into_any_element(),
        }
    }
}

#[cfg(test)]
#[path = "test/auto_update_tests.rs"]
mod tests;
