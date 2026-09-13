use std::{path::PathBuf, sync::Arc};

use gpui::{BackgroundExecutor, Context, Task};
use parking_lot::{Mutex, RwLock};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::{Event, Terminal};

mod platform;

pub(crate) use platform::ProcessIdGetter;

pub(crate) struct PtyProcessInfo {
    system: Mutex<System>,
    refresh_kind: ProcessRefreshKind,
    pid_getter: ProcessIdGetter,
    process_pid: Mutex<Option<u32>>,
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
            process_pid: Mutex::new(
                (pid_getter.fallback_pid().as_u32() > 0)
                    .then_some(pid_getter.fallback_pid().as_u32()),
            ),
            last_foreground_pid: Mutex::new(None),
            current: RwLock::new(None),
            task: Mutex::new(None),
        }
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

    pub(crate) fn kill_current_process(&self, executor: &BackgroundExecutor) {
        let Some(pid) = self.process_pid.lock().take() else {
            return;
        };
        platform::terminate_process_tree(pid, executor);
    }
}
