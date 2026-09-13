use std::process::Command;

#[cfg(windows)]
use std::process::Stdio;

#[cfg(unix)]
pub(super) fn configure_for_tree(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;

    command.process_group(0);
}

#[cfg(not(unix))]
pub(super) fn configure_for_tree(_command: &mut Command) {}

pub(super) fn interrupt(process_id: u32) {
    #[cfg(unix)]
    unsafe {
        // 远程命令运行在独立进程组中，负进程号可以同时覆盖 git、ssh、凭据助手与钩子。
        libc::kill(-(process_id as i32), libc::SIGINT);
    }

    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &process_id.to_string(), "/T"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

pub(super) fn kill(process_id: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(process_id as i32), libc::SIGKILL);
    }

    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &process_id.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}
