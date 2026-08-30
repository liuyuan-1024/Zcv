//! Zcv 退出后执行的 macOS 更新辅助程序。

use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};

fn main() {
    if let Err(error) = run() {
        eprintln!("Zcv 更新失败：{error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let args = parse_args(std::env::args_os().skip(1))?;
        macos::apply_update(&args.transaction_path, args.parent_pid)
    }
    #[cfg(not(target_os = "macos"))]
    bail!("当前平台尚不支持 Zcv 自动更新")
}

struct Args {
    transaction_path: PathBuf,
    parent_pid: u32,
}

fn parse_args(mut args: impl Iterator<Item = std::ffi::OsString>) -> Result<Args> {
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
    Ok(Args {
        transaction_path: transaction_path.context("缺少 --transaction")?,
        parent_pid: parent_pid.context("缺少 --parent-pid")?,
    })
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::CString;
    use std::fs;
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    use anyhow::{Context as _, Result, ensure};
    use zcv_update::{
        APP_EXECUTABLE_RELATIVE_PATH, UpdateResult, UpdateResultStatus, UpdateTransaction,
        atomic_write_json, read_transaction, verify_macos_app,
    };

    const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    pub(super) fn apply_update(transaction_path: &Path, parent_pid: u32) -> Result<()> {
        let transaction = read_transaction(transaction_path)?;
        wait_for_process_exit(parent_pid);
        match apply_transaction(&transaction) {
            Ok(()) => {
                let _ = fs::remove_file(transaction_path);
                let update_result = UpdateResult {
                    transaction_id: transaction.id.clone(),
                    from_version: transaction.from_version.clone(),
                    to_version: transaction.to_version.clone(),
                    status: UpdateResultStatus::Applied,
                    error: None,
                };
                if let Err(error) = atomic_write_json(&transaction.result_path, &update_result) {
                    eprintln!("新版本已启动，但无法记录更新结果：{error:#}");
                }
                Ok(())
            }
            Err(error) => {
                let update_result = UpdateResult {
                    transaction_id: transaction.id.clone(),
                    from_version: transaction.from_version.clone(),
                    to_version: transaction.to_version.clone(),
                    status: UpdateResultStatus::RolledBack,
                    error: Some(format!("{error:#}")),
                };
                if let Err(result_error) =
                    atomic_write_json(&transaction.result_path, &update_result)
                {
                    eprintln!("无法记录更新失败结果：{result_error:#}");
                }
                Err(error)
            }
        }
    }

    fn apply_transaction(transaction: &UpdateTransaction) -> Result<()> {
        let mut fallback_path = transaction.install_path.clone();
        let result = try_apply_transaction(transaction, &mut fallback_path);
        if let Err(error) = result {
            launch(&fallback_path, None).with_context(|| {
                format!(
                    "更新失败，且无法从 {} 重新启动旧版本",
                    fallback_path.display()
                )
            })?;
            return Err(error);
        }
        Ok(())
    }

    fn try_apply_transaction(
        transaction: &UpdateTransaction,
        fallback_path: &mut PathBuf,
    ) -> Result<()> {
        verify_app(transaction)?;
        let candidate_path = candidate_path(transaction)?;
        if candidate_path.exists() {
            fs::remove_dir_all(&candidate_path)
                .with_context(|| format!("无法清理旧更新候选目录 {}", candidate_path.display()))?;
        }

        let copy = Command::new("/usr/bin/ditto")
            .arg(&transaction.staged_app_path)
            .arg(&candidate_path)
            .output()
            .context("无法启动 ditto 复制更新应用")?;
        ensure!(
            copy.status.success(),
            "无法把更新复制到安装目录：{}",
            String::from_utf8_lossy(&copy.stderr).trim()
        );

        let candidate_transaction = UpdateTransaction {
            staged_app_path: candidate_path.clone(),
            ..transaction.clone()
        };
        if let Err(error) = verify_app(&candidate_transaction) {
            let _ = fs::remove_dir_all(&candidate_path);
            return Err(error).context("安装目录中的更新副本验证失败");
        }
        strip_download_metadata(&candidate_path).context("无法清理安装副本的下载元数据")?;

        atomic_swap(&transaction.install_path, &candidate_path).context("无法原子切换 Zcv.app")?;
        *fallback_path = candidate_path.clone();

        let ack_path = transaction
            .result_path
            .with_file_name(format!("ack-{}.json", transaction.id));
        if ack_path.exists() {
            fs::remove_file(&ack_path)
                .with_context(|| format!("无法清理旧启动确认 {}", ack_path.display()))?;
        }

        let mut child = launch(
            &transaction.install_path,
            Some((&transaction.id, &ack_path)),
        )?;
        match wait_for_startup_ack(&mut child, &ack_path) {
            Ok(()) => {
                if let Err(error) = fs::remove_dir_all(&candidate_path) {
                    eprintln!(
                        "新版本已启动，但无法删除旧版本备份 {}：{error}",
                        candidate_path.display()
                    );
                }
                let _ = fs::remove_file(&ack_path);
                Ok(())
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                atomic_swap(&transaction.install_path, &candidate_path)
                    .context("新版本启动失败，且旧版本回滚失败")?;
                *fallback_path = transaction.install_path.clone();
                let _ = fs::remove_dir_all(&candidate_path);
                Err(error).context("新版本未通过启动确认，已恢复旧版本")
            }
        }
    }

    fn candidate_path(transaction: &UpdateTransaction) -> Result<PathBuf> {
        let parent = transaction
            .install_path
            .parent()
            .context("Zcv.app 安装路径没有父目录")?;
        Ok(parent.join(format!(".Zcv.update-{}.app", transaction.id)))
    }

    fn verify_app(transaction: &UpdateTransaction) -> Result<()> {
        verify_macos_app(&transaction.staged_app_path, &transaction.to_version)
    }

    /// 移除下载元数据（quarantine / provenance）。
    ///
    /// 只作用于已经通过清单签名、SHA-256 与代码签名验证的副本；避免 Gatekeeper
    /// 对已验证副本在每次更新后再次要求人工批准。normal 路径下副本通常没有这些
    /// 属性，removexattr 以 ENOATTR 结束并被忽略。
    pub(super) fn strip_download_metadata(root: &Path) -> Result<()> {
        fn strip_one(path: &Path) -> Result<()> {
            for attribute in ["com.apple.quarantine", "com.apple.provenance"] {
                let c_path = CString::new(path.as_os_str().as_bytes()).context("路径包含空字节")?;
                let c_attribute = CString::new(attribute).context("属性名包含空字节")?;
                let result = unsafe { libc::removexattr(c_path.as_ptr(), c_attribute.as_ptr(), 0) };
                if result != 0 {
                    let error = std::io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ENOATTR) {
                        return Err(error).with_context(|| {
                            format!("无法移除 {} 的属性 {attribute}", path.display())
                        });
                    }
                }
            }
            Ok(())
        }
        fn visit(path: &Path) -> Result<()> {
            for entry in
                fs::read_dir(path).with_context(|| format!("无法读取目录 {}", path.display()))?
            {
                let entry = entry.with_context(|| format!("无法读取目录项 {}", path.display()))?;
                let entry_path = entry.path();
                strip_one(&entry_path)?;
                if entry
                    .file_type()
                    .with_context(|| format!("无法读取文件类型 {}", entry_path.display()))?
                    .is_dir()
                {
                    visit(&entry_path)?;
                }
            }
            Ok(())
        }
        strip_one(root)?;
        visit(root)
    }

    pub(super) fn atomic_swap(first: &Path, second: &Path) -> Result<()> {
        let first = CString::new(first.as_os_str().as_bytes()).context("安装路径包含空字节")?;
        let second = CString::new(second.as_os_str().as_bytes()).context("候选路径包含空字节")?;
        let result =
            unsafe { libc::renamex_np(first.as_ptr(), second.as_ptr(), libc::RENAME_SWAP) };
        if result != 0 {
            return Err(std::io::Error::last_os_error()).context("renamex_np(RENAME_SWAP) 失败");
        }
        Ok(())
    }

    fn launch(app: &Path, update: Option<(&str, &Path)>) -> Result<Child> {
        let executable = app.join(APP_EXECUTABLE_RELATIVE_PATH);
        let mut command = Command::new(&executable);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some((transaction_id, ack_path)) = update {
            command.env("ZCV_UPDATE_TRANSACTION_ID", transaction_id);
            command.env("ZCV_UPDATE_ACK_PATH", ack_path);
        }
        command
            .spawn()
            .with_context(|| format!("无法启动 {}", executable.display()))
    }

    fn wait_for_startup_ack(child: &mut Child, ack_path: &Path) -> Result<()> {
        let start = Instant::now();
        while start.elapsed() < STARTUP_TIMEOUT {
            if ack_path.is_file() {
                return Ok(());
            }
            if let Some(status) = child.try_wait().context("无法查询新版本进程状态")? {
                anyhow::bail!("新版本在启动确认前退出：{status}");
            }
            thread::sleep(POLL_INTERVAL);
        }
        anyhow::bail!("等待新版本启动确认超时");
    }

    fn wait_for_process_exit(pid: u32) {
        loop {
            let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
            if result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                break;
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
}

#[cfg(test)]
#[path = "../test/update_helper_tests.rs"]
mod tests;
