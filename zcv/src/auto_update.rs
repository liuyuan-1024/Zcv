//! 应用级自动更新：启动时单次检查、可信下载、TopBar 状态与重启交接。

mod network_client;

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    result,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context as _, Result, ensure};
use gpui::{
    App, AppContext as _, BackgroundExecutor, Context, Entity, Global, Render, Subscription, Task,
    WeakEntity, Window, div,
    http_client::{AsyncBody, HttpClient},
    prelude::*,
};
use gpui_util::new_std_command;
use semver::Version;
use sha2::{Digest as _, Sha256};
use smol::io::{AsyncReadExt as _, AsyncWriteExt as _};
use zcv_actions::RestartToUpdate;
use zcv_settings::config_dir;
use zcv_ui::Button;
use zcv_update::{
    SelectedRelease, UpdateInstallation, UpdateResult, UpdateResultStatus, UpdateTransaction,
    atomic_write_json, extract_verified_archive, prepare_helper, take_update_result,
    update_result_path, verify_and_parse_manifest, verify_app, verify_downloaded,
    verify_downloaded_asset, write_acknowledgement,
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
    installation: UpdateInstallation,
    updates_dir: PathBuf,
}

#[derive(Clone)]
struct PreparedUpdate {
    version: Version,
    staged_app_path: PathBuf,
}

pub(crate) struct UpdateManager {
    status: UpdateStatus,
    config: Option<UpdateConfig>,
    disabled_reason: Option<Arc<str>>,
    prepared: Option<PreparedUpdate>,
    startup_notification: Option<Arc<str>>,
    check_task: Option<Task<()>>,
    manual_requesters: Vec<WeakEntity<Workspace>>,
    last_check_was_manual: bool,
    restart_started: bool,
}

#[derive(Default)]
struct GlobalUpdateManager(Option<Entity<UpdateManager>>);

impl Global for GlobalUpdateManager {}

pub(crate) fn init(cx: &mut App, client_error: Option<String>) {
    let (config, disabled_reason) = if let Some(error) = &client_error {
        (None, Some(Arc::from(error.as_str())))
    } else {
        match UpdateConfig::from_app(cx) {
            Ok(config) => (Some(config), None),
            Err(error) => (None, Some(Arc::from(format!("{error:#}")))),
        }
    };
    let startup_notification = match (client_error, startup_update_notification()) {
        (Some(error), Some(previous)) => Some(Arc::from(format!("{error}\n{previous}"))),
        (Some(error), None) => Some(Arc::from(error)),
        (None, previous) => previous,
    };
    let manager =
        cx.new(|cx| UpdateManager::new(config, disabled_reason, startup_notification, cx));
    cx.set_global(GlobalUpdateManager(Some(manager.clone())));
    manager.update(cx, |manager, cx| manager.check_for_update(cx));
}

fn updates_dir() -> PathBuf {
    config_dir().join("updates")
}

/// 读取并消费上次更新的落盘结果。
///
/// 回滚需要用户反馈；已应用的结果只记录诊断信息，文件由 `take_update_result` 消费。
fn startup_update_notification() -> Option<Arc<str>> {
    match take_update_result(&updates_dir()) {
        Ok(Some(result)) => {
            if result.status == UpdateResultStatus::Applied {
                eprintln!(
                    "自动更新已应用：{} → {}",
                    result.from_version, result.to_version
                );
            }
            update_result_notification(&result)
        }
        Ok(None) => None,
        Err(error) => {
            eprintln!("无法读取上次更新结果：{error:#}");
            None
        }
    }
}

/// 把更新结果映射为一次性用户提示；成功结果不提示。
fn update_result_notification(result: &UpdateResult) -> Option<Arc<str>> {
    match result.status {
        UpdateResultStatus::Applied => None,
        UpdateResultStatus::RolledBack => Some(Arc::from(format!(
            "自动更新失败，已回滚到 {}：{}",
            result.from_version,
            result.error.as_deref().unwrap_or("未知错误")
        ))),
    }
}

pub(crate) fn acknowledge_started_update() -> Result<()> {
    let Some(ack_path) = env::var_os("ZCV_UPDATE_ACK_PATH").map(PathBuf::from) else {
        return Ok(());
    };
    let transaction_id =
        env::var("ZCV_UPDATE_TRANSACTION_ID").context("缺少 ZCV_UPDATE_TRANSACTION_ID")?;
    write_acknowledgement(&updates_dir(), &transaction_id, &ack_path)
}

impl UpdateConfig {
    fn from_app(cx: &App) -> Result<Self> {
        let process_path = cx.app_path().context("无法确定当前应用路径")?;
        let installation = UpdateInstallation::from_process_path(&process_path)?;
        ensure!(
            installation.helper_path().is_file(),
            "当前安装缺少更新辅助程序"
        );
        let current_version = env!("CARGO_PKG_VERSION")
            .parse()
            .context("应用版本不是合法 SemVer")?;
        Ok(Self {
            current_version,
            manifest_url: MANIFEST_URL.to_owned(),
            public_key: UPDATE_PUBLIC_KEY,
            installation,
            updates_dir: config_dir().join("updates"),
        })
    }
}

impl UpdateManager {
    fn new(
        config: Option<UpdateConfig>,
        disabled_reason: Option<Arc<str>>,
        startup_notification: Option<Arc<str>>,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            status: if config.is_some() {
                UpdateStatus::Idle
            } else {
                UpdateStatus::Disabled
            },
            config,
            disabled_reason,
            prepared: None,
            startup_notification,
            check_task: None,
            manual_requesters: Vec::new(),
            last_check_was_manual: false,
            restart_started: false,
        }
    }

    /// 取出启动时读取的上次更新提示；每个进程只会返回一次。
    pub(crate) fn take_startup_notification(&mut self) -> Option<Arc<str>> {
        self.startup_notification.take()
    }

    pub(crate) fn get(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalUpdateManager>()?.0.clone()
    }

    pub(crate) fn status(&self) -> UpdateStatus {
        self.status.clone()
    }

    pub(crate) fn request_check(
        &mut self,
        workspace: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) {
        if let Some(reason) = &self.disabled_reason {
            show_update_toast(
                &workspace,
                ToastKind::Error,
                format!("检查更新失败：{reason}"),
                cx,
            );
            return;
        }
        if let UpdateStatus::ReadyToRestart { version } = &self.status {
            show_update_toast(
                &workspace,
                ToastKind::Info,
                format!("Zcv {version} 已就绪，请点击顶栏“重启以更新”"),
                cx,
            );
            return;
        }
        if self.manual_requesters.contains(&workspace) {
            return;
        }
        self.manual_requesters.push(workspace.clone());
        if self.check_task.is_none() {
            self.check_for_update(cx);
        }
        let message = match &self.status {
            UpdateStatus::Downloading { version, .. } => format!("正在下载 Zcv {version}"),
            _ => "正在检查更新".to_owned(),
        };
        show_update_toast(&workspace, ToastKind::Info, message, cx);
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
                let feedback = check_feedback(&result);
                let requesters = std::mem::take(&mut manager.manual_requesters);
                manager.last_check_was_manual = !requesters.is_empty();
                match result {
                    Ok(CheckOutcome::Current) => manager.status = UpdateStatus::Idle,
                    Ok(CheckOutcome::Prepared(prepared)) => {
                        manager.status = UpdateStatus::ReadyToRestart {
                            version: prepared.version.clone(),
                        };
                        manager.prepared = Some(prepared);
                    }
                    Err(CheckFailure::Transient(_)) => {
                        manager.status = UpdateStatus::Idle;
                    }
                    Err(CheckFailure::Permanent(error)) => {
                        manager.status = UpdateStatus::Failed {
                            message: Arc::from(format!("自动更新产物无效：{error:#}")),
                        };
                    }
                }
                for workspace in requesters {
                    show_update_toast(&workspace, feedback.0, feedback.1.clone(), cx);
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
        let result_path = update_result_path(&config.updates_dir);
        let transaction = UpdateTransaction::new(
            config.current_version.clone(),
            prepared.version.clone(),
            config.installation.app_path().to_path_buf(),
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

        let helper_source = config.installation.helper_path();
        let helper_name = helper_source
            .as_path()
            .file_name()
            .context("更新辅助程序路径无文件名")?;
        let helper_path = transaction_dir.join(helper_name);
        fs::copy(&helper_source, &helper_path).with_context(|| {
            format!(
                "无法复制更新辅助程序 {} → {}",
                helper_source.display(),
                helper_path.display()
            )
        })?;
        prepare_helper(&helper_path)?;

        let log_path = transaction_dir.join("helper.log");
        let log = fs::File::create(&log_path)
            .with_context(|| format!("无法创建更新日志 {}", log_path.display()))?;
        new_std_command(&helper_path)
            .arg("--transaction")
            .arg(&pending_path)
            .arg("--parent-pid")
            .arg(std::process::id().to_string())
            .current_dir(&config.updates_dir)
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

fn check_feedback(result: &result::Result<CheckOutcome, CheckFailure>) -> (ToastKind, String) {
    match result {
        Ok(CheckOutcome::Current) => (
            ToastKind::Info,
            format!("Zcv {} 已是最新版本", env!("CARGO_PKG_VERSION")),
        ),
        Ok(CheckOutcome::Prepared(prepared)) => (
            ToastKind::Info,
            format!("Zcv {} 已就绪，请点击顶栏“重启以更新”", prepared.version),
        ),
        Err(CheckFailure::Transient(error) | CheckFailure::Permanent(error)) => {
            (ToastKind::Error, format!("检查更新失败：{error:#}"))
        }
    }
}

async fn check_and_stage(
    config: UpdateConfig,
    client: Arc<dyn HttpClient>,
    executor: BackgroundExecutor,
    mut on_progress: impl FnMut(Version, u8),
) -> result::Result<CheckOutcome, CheckFailure> {
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
        .select_newer_release(&config.current_version, config.installation.platform_key())
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
        .spawn(async move { verify_app(&app, &version) })
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
) -> result::Result<Vec<u8>, CheckFailure> {
    let mut response = client
        .get(url, AsyncBody::empty(), true)
        .await
        .map_err(CheckFailure::Transient)?;
    if !response.status().is_success() {
        return Err(CheckFailure::Transient(anyhow::anyhow!(
            "获取{label}返回 HTTP {}",
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
) -> result::Result<(), CheckFailure> {
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
            "下载更新返回 HTTP {}",
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
    let actual = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    verify_downloaded(downloaded, &actual, &release.asset).map_err(CheckFailure::Permanent)?;
    smol::fs::rename(&partial, archive_path)
        .await
        .map_err(|error| CheckFailure::Permanent(error.into()))?;
    on_progress(release.version.clone(), 100);
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
        if let Some(message) = manager.update(cx, |manager, _| manager.take_startup_notification())
        {
            show_failure_toast(&workspace, message, cx);
        }
        if let Some(message) = new_failure(&UpdateStatus::Disabled, &status) {
            show_failure_toast(&workspace, message, cx);
        }
        let subscription = cx.observe(&manager, move |button, manager, cx| {
            let manager = manager.read(cx);
            let status = manager.status();
            if let Some(message) = new_failure(&button.status, &status)
                && !manager.last_check_was_manual
            {
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
    show_update_toast(workspace, ToastKind::Error, message.to_string(), cx);
}

fn show_update_toast(
    workspace: &WeakEntity<Workspace>,
    kind: ToastKind,
    message: String,
    cx: &mut App,
) {
    let workspace = workspace.clone();
    cx.defer(move |cx| {
        workspace
            .update(cx, |workspace, cx| {
                workspace.show_toast(kind, message, vec![], Some(Duration::from_secs(8)), cx);
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
