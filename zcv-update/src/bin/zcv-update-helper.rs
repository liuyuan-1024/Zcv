//! Zcv 退出后执行的跨平台更新辅助程序。

use std::{fs, path::PathBuf};

use anyhow::{Context as _, Result, bail};
use zcv_update::{
    UpdateResult, UpdateResultStatus, UpdateTransaction, atomic_write_json, read_transaction,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("Zcv 更新失败：{error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = parse_args(std::env::args_os().skip(1))?;
    #[cfg(target_os = "macos")]
    {
        apply_update(
            &args,
            macos::wait_for_process_exit,
            macos::apply_transaction,
        )?;
    }
    #[cfg(target_os = "windows")]
    {
        apply_update(
            &args,
            windows::wait_for_process_exit,
            windows::apply_transaction,
        )?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    bail!("当前平台尚不支持 Zcv 自动更新");

    Ok(())
}

fn apply_update(
    args: &Args,
    wait_for_process_exit: impl FnOnce(u32) -> Result<()>,
    apply_transaction: impl FnOnce(&UpdateTransaction) -> Result<()>,
) -> Result<()> {
    let transaction = read_transaction(&args.transaction_path)?;
    wait_for_process_exit(args.parent_pid)?;
    match apply_transaction(&transaction) {
        Ok(()) => {
            let _ = fs::remove_file(&args.transaction_path);
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
            if let Err(result_error) = atomic_write_json(&transaction.result_path, &update_result) {
                eprintln!("无法记录更新失败结果：{result_error:#}");
            }
            Err(error)
        }
    }
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
    use zcv_update::{UpdateTransaction, application_executable_path, verify_app};

    const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    pub(super) fn apply_transaction(transaction: &UpdateTransaction) -> Result<()> {
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
        verify_transaction_app(transaction)?;
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
        if let Err(error) = verify_transaction_app(&candidate_transaction) {
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

    fn verify_transaction_app(transaction: &UpdateTransaction) -> Result<()> {
        verify_app(&transaction.staged_app_path, &transaction.to_version)
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
        let executable = application_executable_path(app);
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

    pub(super) fn wait_for_process_exit(pid: u32) -> Result<()> {
        loop {
            let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
            if result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                break;
            }
            thread::sleep(POLL_INTERVAL);
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    use anyhow::{Context as _, Result, ensure};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Threading::{
        INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    use zcv_update::{UpdateTransaction, application_executable_path, verify_app};

    const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    pub(super) fn apply_transaction(transaction: &UpdateTransaction) -> Result<()> {
        let backup_path = backup_path(transaction)?;
        match try_apply_transaction(transaction, &backup_path) {
            Ok(()) => Ok(()),
            Err(error) => {
                launch(&transaction.install_path, None).with_context(|| {
                    format!(
                        "更新失败，且无法从 {} 重新启动旧版本",
                        transaction.install_path.display()
                    )
                })?;
                Err(error)
            }
        }
    }

    fn try_apply_transaction(transaction: &UpdateTransaction, backup_path: &Path) -> Result<()> {
        verify_app(&transaction.staged_app_path, &transaction.to_version)?;
        let candidate_path = candidate_path(transaction)?;
        if candidate_path.exists() {
            fs::remove_dir_all(&candidate_path)
                .with_context(|| format!("无法清理旧更新候选目录 {}", candidate_path.display()))?;
        }
        copy_dir(&transaction.staged_app_path, &candidate_path)?;
        let candidate_transaction = UpdateTransaction {
            staged_app_path: candidate_path.clone(),
            ..transaction.clone()
        };
        if let Err(error) = verify_app(
            &candidate_transaction.staged_app_path,
            &candidate_transaction.to_version,
        ) {
            let _ = fs::remove_dir_all(&candidate_path);
            return Err(error).context("安装目录中的更新副本验证失败");
        }

        if backup_path.exists() {
            fs::remove_dir_all(backup_path)
                .with_context(|| format!("无法清理旧版本备份 {}", backup_path.display()))?;
        }
        fs::rename(&transaction.install_path, backup_path).with_context(|| {
            format!(
                "无法移动当前安装目录 {} → {}",
                transaction.install_path.display(),
                backup_path.display()
            )
        })?;
        if let Err(error) = fs::rename(&candidate_path, &transaction.install_path) {
            let _ = fs::rename(backup_path, &transaction.install_path);
            return Err(error).context("无法把更新目录移动到安装位置");
        }

        let ack_path = transaction
            .result_path
            .with_file_name(format!("ack-{}.json", transaction.id));
        if let Err(error) = ack_path
            .exists()
            .then(|| fs::remove_file(&ack_path))
            .transpose()
        {
            rollback_after_switch(transaction, backup_path, &ack_path, None)?;
            return Err(error)
                .with_context(|| format!("无法清理旧启动确认 {}", ack_path.display()));
        }
        let mut child = match launch(
            &transaction.install_path,
            Some((&transaction.id, &ack_path)),
        ) {
            Ok(child) => child,
            Err(error) => {
                rollback_after_switch(transaction, backup_path, &ack_path, None)
                    .context("新版本启动失败，且旧版本回滚失败")?;
                return Err(error).context("新版本启动失败，已恢复旧版本");
            }
        };
        match wait_for_startup_ack(&mut child, &ack_path) {
            Ok(()) => {
                if let Err(error) = fs::remove_dir_all(backup_path) {
                    eprintln!(
                        "新版本已启动，但无法删除旧版本备份 {}：{error}",
                        backup_path.display()
                    );
                }
                let _ = fs::remove_file(&ack_path);
                Ok(())
            }
            Err(error) => {
                rollback_after_switch(transaction, backup_path, &ack_path, Some(&mut child))
                    .context("新版本启动失败，且旧版本回滚失败")?;
                Err(error).context("新版本未通过启动确认，已恢复旧版本")
            }
        }
    }

    fn rollback_after_switch(
        transaction: &UpdateTransaction,
        backup_path: &Path,
        ack_path: &Path,
        mut child: Option<&mut Child>,
    ) -> Result<()> {
        if let Some(child) = child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        fs::remove_dir_all(&transaction.install_path)
            .context("新版本启动失败，且无法移除新版本")?;
        fs::rename(backup_path, &transaction.install_path)
            .context("新版本启动失败，且旧版本回滚失败")?;
        let _ = fs::remove_file(ack_path);
        Ok(())
    }

    fn copy_dir(source: &Path, destination: &Path) -> Result<()> {
        fs::create_dir_all(destination)
            .with_context(|| format!("无法创建更新目录 {}", destination.display()))?;
        for entry in fs::read_dir(source)
            .with_context(|| format!("无法读取更新目录 {}", source.display()))?
        {
            let entry = entry.with_context(|| format!("无法读取目录项 {}", source.display()))?;
            let source_path = entry.path();
            let destination_path = destination.join(entry.file_name());
            if entry
                .file_type()
                .with_context(|| format!("无法读取文件类型 {}", source_path.display()))?
                .is_dir()
            {
                copy_dir(&source_path, &destination_path)?;
            } else {
                fs::copy(&source_path, &destination_path).with_context(|| {
                    format!(
                        "无法复制更新文件 {} → {}",
                        source_path.display(),
                        destination_path.display()
                    )
                })?;
            }
        }
        Ok(())
    }

    fn candidate_path(transaction: &UpdateTransaction) -> Result<PathBuf> {
        let parent = transaction
            .install_path
            .parent()
            .context("安装路径没有父目录")?;
        Ok(parent.join(format!(".Zcv.update-{}", transaction.id)))
    }

    fn backup_path(transaction: &UpdateTransaction) -> Result<PathBuf> {
        let parent = transaction
            .install_path
            .parent()
            .context("安装路径没有父目录")?;
        Ok(parent.join(format!(".Zcv.backup-{}", transaction.id)))
    }

    fn launch(app: &Path, update: Option<(&str, &Path)>) -> Result<Child> {
        let executable = application_executable_path(app);
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
        anyhow::bail!("等待新版本启动确认超时")
    }

    pub(super) fn wait_for_process_exit(pid: u32) -> Result<()> {
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            ensure!(
                unsafe { GetLastError() } == ERROR_INVALID_PARAMETER,
                "无法打开 Zcv 进程以等待退出"
            );
            return Ok(());
        }
        let result = unsafe { WaitForSingleObject(handle, INFINITE) };
        unsafe { CloseHandle(handle) };
        ensure!(result == WAIT_OBJECT_0, "等待 Zcv 退出失败");
        Ok(())
    }
}

#[cfg(test)]
#[path = "../test/update_helper_tests.rs"]
mod tests;
