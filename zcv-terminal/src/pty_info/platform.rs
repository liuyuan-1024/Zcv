use gpui::BackgroundExecutor;
use sysinfo::Pid;

#[derive(Clone, Copy)]
pub(crate) struct ProcessIdGetter {
    #[cfg(unix)]
    handle: i32,
    fallback_pid: u32,
}

impl ProcessIdGetter {
    #[cfg(unix)]
    pub(crate) fn new(handle: i32, fallback_pid: u32) -> Self {
        Self {
            handle,
            fallback_pid,
        }
    }

    #[cfg(windows)]
    pub(crate) fn new(fallback_pid: u32) -> Self {
        Self { fallback_pid }
    }

    pub(super) fn fallback_pid(&self) -> Pid {
        Pid::from_u32(self.fallback_pid)
    }

    #[cfg(unix)]
    pub(super) fn foreground_pid(&self) -> Option<Pid> {
        let pid = unsafe { libc::tcgetpgrp(self.handle) };
        if pid > 0 {
            Some(Pid::from_u32(pid as u32))
        } else if self.fallback_pid > 0 {
            Some(self.fallback_pid())
        } else {
            None
        }
    }

    #[cfg(windows)]
    pub(super) fn foreground_pid(&self) -> Option<Pid> {
        (self.fallback_pid > 0).then(|| self.fallback_pid())
    }
}

pub(super) fn terminate_process_tree(pid: u32, executor: &BackgroundExecutor) {
    #[cfg(unix)]
    {
        use std::time::Duration;

        const PROCESS_KILL_GRACE_PERIOD: Duration = Duration::from_millis(100);
        let pid = pid as i32;
        // 先终止进程组，宽限期后再强制终止。
        unsafe {
            libc::killpg(pid, libc::SIGTERM);
        }
        let timer = executor.clone();
        executor
            .spawn(async move {
                timer.timer(PROCESS_KILL_GRACE_PERIOD).await;
                unsafe {
                    libc::killpg(pid, libc::SIGKILL);
                }
            })
            .detach();
    }

    #[cfg(windows)]
    {
        let _ = executor;
        // ConPTY 不会因关闭事件循环而可靠终止 shell 的子进程树。
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .status();
    }
}
