mod support;

use support::{ExampleAction, ExampleState};
use zom_engine::*;

fn main() {
    let state =
        SelectionMovementState::new().expect("selection movement composition example should init");
    support::run_interactive_example(
        "Selection / Movement / Composition",
        "点击动作体验 SelectionSet、多光标移动和 IME composition 状态流。",
        state,
        vec![
            ExampleAction {
                label: "Multi-cursor Insert",
                detail: "在 hello 和 world 前放两个 caret 并同时插入 >。",
                run: SelectionMovementState::insert_at_two_carets,
            },
            ExampleAction {
                label: "Move Word",
                detail: "让当前 selection 按 word 语义向后移动。",
                run: SelectionMovementState::move_word,
            },
            ExampleAction {
                label: "Commit Composition",
                detail: "启动、更新并提交一次中文 composition。",
                run: SelectionMovementState::commit_composition,
            },
            ExampleAction {
                label: "Delete Backward",
                detail: "复用 selection 删除管线处理 caret 前字符。",
                run: SelectionMovementState::delete_backward,
            },
        ],
    );
}

struct SelectionMovementState {
    buffer: Buffer,
    last_moved: Vec<TextRange>,
}

impl SelectionMovementState {
    fn new() -> EngineResult<Self> {
        Ok(Self {
            buffer: Buffer::from_text("hello world".to_string(), BufferConfig::default())?,
            last_moved: Vec::new(),
        })
    }

    fn insert_at_two_carets(&mut self) -> Result<String, String> {
        let selections = SelectionSet::new(vec![
            Selection::caret(ByteOffset::new(0)),
            Selection::caret(ByteOffset::new(6)),
        ]);
        self.buffer
            .insert_at_selections(selections, ">")
            .map_err(err)?;
        Ok(format!(
            "多光标插入后文本为 {:?}",
            self.buffer.text().as_ref()
        ))
    }

    fn move_word(&mut self) -> Result<String, String> {
        let moved = self
            .buffer
            .move_current_selection(MovementDirection::Next, MovementUnit::Word, false)
            .map_err(err)?;
        self.last_moved = moved.ranges();
        Ok(format!("移动后 selection = {:?}", self.last_moved))
    }

    fn commit_composition(&mut self) -> Result<String, String> {
        self.buffer
            .set_selection(SelectionSet::caret(self.buffer.len_bytes()))
            .map_err(err)?;
        self.buffer.start_composition().map_err(err)?;
        self.buffer.update_composition(" 输入", None).map_err(err)?;
        self.buffer.commit_composition(" 输入").map_err(err)?;
        Ok(format!(
            "composition 提交后文本为 {:?}",
            self.buffer.text().as_ref()
        ))
    }

    fn delete_backward(&mut self) -> Result<String, String> {
        let selections = self.buffer.selection().clone();
        self.buffer
            .delete_backward_at_selections(selections)
            .map_err(err)?;
        Ok(format!(
            "delete backward 后文本为 {:?}",
            self.buffer.text().as_ref()
        ))
    }
}

impl ExampleState for SelectionMovementState {
    fn facts(&self) -> Vec<String> {
        vec![
            format!("version = {:?}", self.buffer.version()),
            format!("selection ranges = {:?}", self.buffer.selection().ranges()),
            format!("last moved ranges = {:?}", self.last_moved),
            format!("is_composing = {}", self.buffer.is_composing()),
            format!("undo_depth = {}", self.buffer.history_status().undo_depth),
        ]
    }

    fn document(&self) -> Option<String> {
        Some(self.buffer.text().to_string())
    }
}

fn err(error: impl ToString) -> String {
    error.to_string()
}
