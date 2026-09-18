//! 更新 helper 的平台无关事务编排。
//!
//! 平台后端只提供路径布局、原子切换/回滚原语和进程退出等待；
//! 暂存校验、切换顺序、启动确认、清理与结果落盘在这里统一实现。

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

use crate::platform::{ApplyBackend, StartedProcess};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use crate::{UpdateResult, UpdateResultStatus, atomic_write_json, read_transaction};
use crate::{UpdateTransaction, acknowledgement_path};

/// helper 的命令行参数。
pub(crate) struct HelperArgs {
    pub transaction_path: PathBuf,
    pub parent_pid: u32,
}

/// helper 的公共入口：解析参数并在应用退出后应用更新事务。
pub fn run_helper(args: impl Iterator<Item = OsString>) -> Result<()> {
    let args = parse_helper_args(args)?;
    apply_update(&args.transaction_path, args.parent_pid)
}

pub(crate) fn parse_helper_args(mut args: impl Iterator<Item = OsString>) -> Result<HelperArgs> {
    let mut transaction_path = None;
    let mut parent_pid = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--transaction") => {
                transaction_path = Some(args.next().context("--transaction 缺少路径")?.into());
            }
            Some("--parent-pid") => {
                parent_pid = Some(
                    args.next()
                        .context("--parent-pid 缺少进程 ID")?
                        .to_str()
                        .context("进程 ID 不是 UTF-8")?
                        .parse()
                        .context("进程 ID 无效")?,
                );
            }
            _ => bail!("未知参数 {}", arg.to_string_lossy()),
        }
    }
    Ok(HelperArgs {
        transaction_path: transaction_path.context("缺少 --transaction")?,
        parent_pid: parent_pid.context("缺少 --parent-pid")?,
    })
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn apply_update(transaction_path: &Path, parent_pid: u32) -> Result<()> {
    let transaction = read_transaction(transaction_path)?;
    let backend = crate::platform::backend();
    backend.wait_for_process_exit(parent_pid)?;
    match run_apply(&transaction, &backend) {
        Ok(()) => {
            let _ = fs::remove_file(transaction_path);
            if let Err(error) = write_result(&transaction, UpdateResultStatus::Applied, None) {
                eprintln!("新版本已启动，但无法记录更新结果：{error:#}");
            }
            Ok(())
        }
        Err(error) => {
            let relaunch = backend.launch(&transaction.install_path, None).map(|_| ());
            let message = match &relaunch {
                Ok(()) => format!("{error:#}"),
                Err(relaunch_error) => format!(
                    "更新失败，且无法从 {} 重新启动旧版本：{relaunch_error:#}",
                    transaction.install_path.display()
                ),
            };
            if let Err(result_error) =
                write_result(&transaction, UpdateResultStatus::RolledBack, Some(message))
            {
                eprintln!("无法记录更新失败结果：{result_error:#}");
            }
            match relaunch {
                Ok(()) => Err(error),
                Err(relaunch_error) => Err(relaunch_error),
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn apply_update(_transaction_path: &Path, _parent_pid: u32) -> Result<()> {
    bail!("当前平台尚不支持 Zcv 自动更新")
}

/// 平台无关事务流程：暂存、切换、启动、启动确认、清理或回滚。
///
/// 切换失败时旧版本仍在安装目录；切换到新版本后任何失败都会先回滚再返回错误。
pub(crate) fn run_apply<B: ApplyBackend + ?Sized>(
    transaction: &UpdateTransaction,
    backend: &B,
) -> Result<()> {
    let staged = backend.stage(transaction)?;
    if let Err(error) = backend.switch(transaction, &staged) {
        backend.cleanup(transaction, &staged);
        return Err(error);
    }
    let directory = transaction
        .result_path
        .parent()
        .context("更新结果路径没有父目录")?;
    let ack_path = acknowledgement_path(directory, &transaction.id);
    if ack_path.exists()
        && let Err(error) = fs::remove_file(&ack_path)
    {
        backend.cleanup(transaction, &staged);
        backend
            .rollback(transaction, &staged)
            .context("无法清理旧启动确认，且旧版本回滚失败")?;
        return Err(error).with_context(|| format!("无法清理旧启动确认 {}", ack_path.display()));
    }
    let mut child = match backend.launch(
        &transaction.install_path,
        Some((&transaction.id, &ack_path)),
    ) {
        Ok(child) => child,
        Err(error) => {
            backend
                .rollback(transaction, &staged)
                .context("新版本启动失败，且旧版本回滚失败")?;
            backend.cleanup(transaction, &staged);
            return Err(error).context("新版本启动失败，已恢复旧版本");
        }
    };
    match wait_for_startup_ack(
        child.as_mut(),
        &ack_path,
        backend.startup_timeout(),
        backend.poll_interval(),
    ) {
        Ok(()) => {
            backend.cleanup(transaction, &staged);
            let _ = fs::remove_file(&ack_path);
            Ok(())
        }
        Err(error) => {
            child.kill();
            child.wait();
            backend
                .rollback(transaction, &staged)
                .context("新版本启动失败，且旧版本回滚失败")?;
            backend.cleanup(transaction, &staged);
            let _ = fs::remove_file(&ack_path);
            Err(error).context("新版本未通过启动确认，已恢复旧版本")
        }
    }
}

fn wait_for_startup_ack(
    child: &mut dyn StartedProcess,
    ack_path: &Path,
    timeout: Duration,
    poll_interval: Duration,
) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if ack_path.is_file() {
            return Ok(());
        }
        if let Some(status) = child.exit_status()? {
            bail!("新版本在启动确认前退出：{status}");
        }
        thread::sleep(poll_interval);
    }
    bail!("等待新版本启动确认超时")
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn write_result(
    transaction: &UpdateTransaction,
    status: UpdateResultStatus,
    error: Option<String>,
) -> Result<()> {
    let result = UpdateResult {
        transaction_id: transaction.id.clone(),
        from_version: transaction.from_version.clone(),
        to_version: transaction.to_version.clone(),
        status,
        error,
    };
    atomic_write_json(&transaction.result_path, &result)
}
