mod support;

use support::{ExampleAction, ExampleState};
use zom_engine::*;

fn main() {
    let state = TransactionsHistoryState::new().expect("transactions history example should init");
    support::run_interactive_example(
        "Transactions / History",
        "点击动作逐步体验 Transaction、Delta、PositionMap、Undo / Redo 的状态演进。",
        state,
        vec![
            ExampleAction {
                label: "Apply Transaction",
                detail: "在 byte 1 插入 XYZ，提交绑定当前版本的事务。",
                run: TransactionsHistoryState::apply_insert,
            },
            ExampleAction {
                label: "Undo",
                detail: "通过历史系统回退文本和版本状态。",
                run: TransactionsHistoryState::undo,
            },
            ExampleAction {
                label: "Redo",
                detail: "通过历史系统恢复刚才回退的事务。",
                run: TransactionsHistoryState::redo,
            },
            ExampleAction {
                label: "Replace Head",
                detail: "用普通编辑入口产生新的历史节点。",
                run: TransactionsHistoryState::replace_head,
            },
        ],
    );
}

struct TransactionsHistoryState {
    buffer: Buffer,
    last_delta: Option<(BufferVersion, BufferVersion)>,
    last_mapped_end: Option<ByteOffset>,
}

impl TransactionsHistoryState {
    fn new() -> EngineResult<Self> {
        Ok(Self {
            buffer: Buffer::from_text("abc".to_string(), BufferConfig::default())?,
            last_delta: None,
            last_mapped_end: None,
        })
    }

    fn apply_insert(&mut self) -> Result<String, String> {
        let tx = Transaction::from_edits(
            self.buffer.version(),
            vec![Edit::insert(ByteOffset::new(1), "XYZ".to_string()).map_err(err)?],
        )
        .map_err(err)?;
        let old_end = self.buffer.len_bytes();
        let (delta, changeset) = self.buffer.apply_transaction(tx).map_err(err)?;
        self.last_delta = Some((delta.old_version(), delta.new_version()));
        self.last_mapped_end = Some(changeset.position_map().map_old_position(old_end).value());
        Ok(format!(
            "提交事务后文本变为 {:?}",
            self.buffer.text().as_ref()
        ))
    }

    fn undo(&mut self) -> Result<String, String> {
        match self.buffer.undo().map_err(err)? {
            Some(_) => Ok(format!("undo 后文本为 {:?}", self.buffer.text().as_ref())),
            None => Ok("没有可 undo 的历史节点".to_string()),
        }
    }

    fn redo(&mut self) -> Result<String, String> {
        match self.buffer.redo().map_err(err)? {
            Some(_) => Ok(format!("redo 后文本为 {:?}", self.buffer.text().as_ref())),
            None => Ok("没有可 redo 的历史节点".to_string()),
        }
    }

    fn replace_head(&mut self) -> Result<String, String> {
        let end = ByteOffset::new(self.buffer.len_bytes().get().min(1));
        let range = TextRange::new(ByteOffset::ZERO, end).map_err(err)?;
        self.buffer.replace(range, "@").map_err(err)?;
        Ok(format!(
            "替换头部后文本为 {:?}",
            self.buffer.text().as_ref()
        ))
    }
}

impl ExampleState for TransactionsHistoryState {
    fn facts(&self) -> Vec<String> {
        let history = self.buffer.history_status();
        let mut facts = vec![
            format!("version = {:?}", self.buffer.version()),
            format!("undo_depth = {}", history.undo_depth),
            format!("redo_depth = {}", history.redo_depth),
            format!(
                "can_undo / can_redo = {} / {}",
                self.buffer.can_undo(),
                self.buffer.can_redo()
            ),
        ];

        if let Some((old_version, new_version)) = self.last_delta {
            facts.push(format!(
                "last delta = {:?} -> {:?}",
                old_version, new_version
            ));
        }
        if let Some(mapped) = self.last_mapped_end {
            facts.push(format!("last old end mapped to byte {}", mapped));
        }

        facts
    }

    fn document(&self) -> Option<String> {
        Some(self.buffer.text().to_string())
    }
}

fn err(error: impl ToString) -> String {
    error.to_string()
}
