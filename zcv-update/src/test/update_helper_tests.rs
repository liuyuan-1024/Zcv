use std::cell::RefCell;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};

use crate::helper::{parse_helper_args, run_apply};
use crate::platform::{APP_DIRECTORY_NAME, ApplyBackend, StagedUpdate, StartedProcess};
use crate::{UpdateTransaction, update_result_path};

#[test]
fn required_arguments_are_parsed() {
    let args = parse_helper_args(
        ["--transaction", "/tmp/pending.json", "--parent-pid", "42"]
            .into_iter()
            .map(OsString::from),
    )
    .unwrap();
    assert_eq!(args.transaction_path, PathBuf::from("/tmp/pending.json"));
    assert_eq!(args.parent_pid, 42);
}

struct FakeProcess {
    exited: bool,
}

impl StartedProcess for FakeProcess {
    fn exit_status(&mut self) -> Result<Option<String>> {
        Ok(self.exited.then(|| "已退出".to_owned()))
    }

    fn kill(&mut self) {
        self.exited = true;
    }

    fn wait(&mut self) {
        self.exited = true;
    }
}

/// 记录状态转移并可控成功/失败/确认超时的平台后端替身。
struct FakeBackend {
    switch_fails: bool,
    ack_timeout: bool,
    events: RefCell<Vec<&'static str>>,
    candidate: PathBuf,
}

impl FakeBackend {
    fn new(root: &Path, switch_fails: bool, ack_timeout: bool) -> Self {
        Self {
            switch_fails,
            ack_timeout,
            events: RefCell::new(Vec::new()),
            candidate: root.join("candidate"),
        }
    }

    fn events(&self) -> Vec<&'static str> {
        self.events.borrow().clone()
    }
}

impl ApplyBackend for FakeBackend {
    fn wait_for_process_exit(&self, _pid: u32) -> Result<()> {
        self.events.borrow_mut().push("wait");
        Ok(())
    }

    fn stage(&self, _transaction: &UpdateTransaction) -> Result<StagedUpdate> {
        self.events.borrow_mut().push("stage");
        Ok(StagedUpdate {
            candidate_path: self.candidate.clone(),
            previous_path: self.candidate.clone(),
        })
    }

    fn switch(&self, _transaction: &UpdateTransaction, _staged: &StagedUpdate) -> Result<()> {
        self.events.borrow_mut().push("switch");
        if self.switch_fails {
            bail!("切换失败");
        }
        Ok(())
    }

    fn rollback(&self, _transaction: &UpdateTransaction, _staged: &StagedUpdate) -> Result<()> {
        self.events.borrow_mut().push("rollback");
        Ok(())
    }

    fn cleanup(&self, _transaction: &UpdateTransaction, _staged: &StagedUpdate) {
        self.events.borrow_mut().push("cleanup");
    }

    fn launch(
        &self,
        _app: &Path,
        update: Option<(&str, &Path)>,
    ) -> Result<Box<dyn StartedProcess>> {
        self.events.borrow_mut().push("launch");
        if let Some((_, ack_path)) = update
            && !self.ack_timeout
        {
            std::fs::write(ack_path, b"{}").unwrap();
        }
        Ok(Box::new(FakeProcess { exited: false }))
    }

    fn startup_timeout(&self) -> Duration {
        Duration::from_millis(20)
    }

    fn poll_interval(&self) -> Duration {
        Duration::from_millis(1)
    }
}

fn transaction(root: &Path) -> UpdateTransaction {
    let updates = root.join("updates");
    std::fs::create_dir_all(&updates).unwrap();
    let install = root.join(APP_DIRECTORY_NAME);
    std::fs::create_dir_all(&install).unwrap();
    let staged = root.join("staged").join(APP_DIRECTORY_NAME);
    std::fs::create_dir_all(&staged).unwrap();
    UpdateTransaction::new(
        "1.0.0".parse().unwrap(),
        "1.0.1".parse().unwrap(),
        install,
        staged,
        update_result_path(&updates),
    )
    .unwrap()
}

#[test]
fn successful_transaction_cleans_up_after_startup_ack() {
    let root = tempfile::tempdir().unwrap();
    let transaction = transaction(root.path());
    let backend = FakeBackend::new(root.path(), false, false);

    run_apply(&transaction, &backend).unwrap();

    assert_eq!(
        backend.events(),
        vec!["stage", "switch", "launch", "cleanup"]
    );
    let ack_path =
        crate::acknowledgement_path(root.path().join("updates").as_path(), &transaction.id);
    assert!(!ack_path.exists(), "启动确认应已被清理");
}

#[test]
fn switch_failure_cleans_up_without_launching() {
    let root = tempfile::tempdir().unwrap();
    let transaction = transaction(root.path());
    let backend = FakeBackend::new(root.path(), true, false);

    assert!(run_apply(&transaction, &backend).is_err());

    assert_eq!(backend.events(), vec!["stage", "switch", "cleanup"]);
}

#[test]
fn startup_ack_timeout_rolls_back_and_cleans_up() {
    let root = tempfile::tempdir().unwrap();
    let transaction = transaction(root.path());
    let backend = FakeBackend::new(root.path(), false, true);

    assert!(run_apply(&transaction, &backend).is_err());

    assert_eq!(
        backend.events(),
        vec!["stage", "switch", "launch", "rollback", "cleanup"]
    );
}
