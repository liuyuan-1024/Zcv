use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use gpui::{BackgroundExecutor, Context, Task};
use parking_lot::{Mutex, RwLock};

use crate::{Event, Terminal};

mod platform;

pub(crate) use platform::ProcessIdGetter;

pub(crate) struct PtyProcessInfo {
    cwd_reader: Mutex<platform::ProcessCwdReader>,
    pid_getter: ProcessIdGetter,
    shell_pid: Mutex<Option<u32>>,
    current: RwLock<Option<PathBuf>>,
    task: Mutex<Option<Task<()>>>,
}

impl PtyProcessInfo {
    pub(crate) fn new(pid_getter: ProcessIdGetter) -> Self {
        Self {
            cwd_reader: Mutex::new(platform::ProcessCwdReader::new()),
            pid_getter,
            shell_pid: Mutex::new(
                (pid_getter.fallback_pid() > 0).then_some(pid_getter.fallback_pid()),
            ),
            current: RwLock::new(None),
            task: Mutex::new(None),
        }
    }

    fn load(&self) -> Option<PathBuf> {
        let foreground_pid = self.pid_getter.foreground_pid()?;
        self.cwd_reader.lock().read_cwd(foreground_pid)
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

    pub(crate) fn terminate_process_tree(&self, executor: &BackgroundExecutor) -> Result<()> {
        let Some(pid) = self.shell_pid.lock().as_ref().copied() else {
            return Ok(());
        };
        platform::terminate_process_tree(pid, executor)?;
        self.shell_pid.lock().take();
        Ok(())
    }

    pub(crate) fn mark_exited(&self) {
        self.shell_pid.lock().take();
    }
}
