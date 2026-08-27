use std::{path::PathBuf, sync::Arc};

use gpui::{Context, Task};
use parking_lot::{Mutex, RwLock};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::{Event, Terminal};

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

    pub(crate) fn fallback_pid(&self) -> Pid {
        Pid::from_u32(self.fallback_pid)
    }

    #[cfg(unix)]
    fn foreground_pid(&self) -> Option<Pid> {
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
    fn foreground_pid(&self) -> Option<Pid> {
        (self.fallback_pid > 0).then(|| self.fallback_pid())
    }
}

pub(crate) struct PtyProcessInfo {
    system: Mutex<System>,
    refresh_kind: ProcessRefreshKind,
    pid_getter: ProcessIdGetter,
    last_foreground_pid: Mutex<Option<Pid>>,
    current: RwLock<Option<PathBuf>>,
    task: Mutex<Option<Task<()>>>,
}

impl PtyProcessInfo {
    pub(crate) fn new(pid_getter: ProcessIdGetter) -> Self {
        let refresh_kind = ProcessRefreshKind::nothing()
            .with_cwd(UpdateKind::Always)
            .without_tasks();
        Self {
            system: Mutex::new(System::new()),
            refresh_kind,
            pid_getter,
            last_foreground_pid: Mutex::new(None),
            current: RwLock::new(None),
            task: Mutex::new(None),
        }
    }

    #[cfg(all(test, unix))]
    pub(crate) fn foreground_pid(&self) -> Option<Pid> {
        self.pid_getter.foreground_pid()
    }

    fn load(&self) -> Option<PathBuf> {
        let foreground_pid = self.pid_getter.foreground_pid()?;
        let mut system = self.system.lock();
        if self.last_foreground_pid.lock().replace(foreground_pid) != Some(foreground_pid) {
            *system = System::new();
        }
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[foreground_pid]),
            true,
            self.refresh_kind,
        );
        system.process(foreground_pid)?.cwd().map(PathBuf::from)
    }

    pub(crate) fn refresh(self: &Arc<Self>, cx: &mut Context<Terminal>) {
        if self.task.lock().is_some() {
            return;
        }
        let process_info = Arc::clone(self);
        let refresh_task = cx.background_executor().spawn(async move {
            let previous = process_info.current.read().clone();
            let current = process_info.load();
            if current != previous {
                *process_info.current.write() = current.clone();
            }
            current.filter(|current| Some(current) != previous.as_ref())
        });
        let process_info = Arc::downgrade(self);
        *self.task.lock() = Some(cx.spawn(async move |terminal, cx| {
            if let Some(working_directory) = refresh_task.await {
                terminal
                    .update(cx, |terminal, cx| {
                        terminal.cwd = Some(working_directory);
                        cx.emit(Event::TitleChanged(terminal.title.clone()));
                        cx.notify();
                    })
                    .ok();
            }
            if let Some(process_info) = process_info.upgrade() {
                process_info.task.lock().take();
            }
        }));
    }
}
