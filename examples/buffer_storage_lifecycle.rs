mod support;

use support::{ExampleAction, ExampleState};
use zom_engine::*;

fn main() {
    let state = BufferLifecycleState::new().expect("buffer lifecycle example should init");
    support::run_interactive_example(
        "Buffer / Storage / Lifecycle",
        "点击动作体验 Buffer 文本状态、保存边界、Snapshot 和 reload 状态迁移。",
        state,
        vec![
            ExampleAction {
                label: "Insert Text",
                detail: "在 hello 后插入 , engine，观察 dirty 和版本推进。",
                run: BufferLifecycleState::insert_text,
            },
            ExampleAction {
                label: "Capture Snapshot",
                detail: "保存只读快照，后续 reload 后仍可观察旧文本。",
                run: BufferLifecycleState::capture_snapshot,
            },
            ExampleAction {
                label: "Mark Saved",
                detail: "模拟保存边界，把当前版本标为 saved。",
                run: BufferLifecycleState::mark_saved,
            },
            ExampleAction {
                label: "Reload",
                detail: "从外部文本 reload，重置文本和 dirty 状态。",
                run: BufferLifecycleState::reload,
            },
        ],
    );
}

struct BufferLifecycleState {
    buffer: Buffer,
    snapshot_text: Option<String>,
    snapshot_version: Option<BufferVersion>,
}

impl BufferLifecycleState {
    fn new() -> EngineResult<Self> {
        Ok(Self {
            buffer: Buffer::with_origin(
                BufferOrigin::external("memory://example/buffer"),
                "hello\nworld".to_string(),
                BufferConfig::default(),
            )?,
            snapshot_text: None,
            snapshot_version: None,
        })
    }

    fn insert_text(&mut self) -> Result<String, String> {
        self.buffer
            .insert(ByteOffset::new(5), ", engine")
            .map_err(err)?;
        Ok(format!("插入后文本为 {:?}", self.buffer.text().as_ref()))
    }

    fn capture_snapshot(&mut self) -> Result<String, String> {
        let snapshot = self.buffer.snapshot();
        self.snapshot_text = Some(snapshot.text().to_string());
        self.snapshot_version = Some(snapshot.version());
        Ok(format!("捕获 snapshot {:?}", snapshot.version()))
    }

    fn mark_saved(&mut self) -> Result<String, String> {
        self.buffer.mark_saved();
        Ok(format!("saved_version = {:?}", self.buffer.saved_version()))
    }

    fn reload(&mut self) -> Result<String, String> {
        self.buffer
            .reload_from_text("fresh\nstate".to_string())
            .map_err(err)?;
        Ok(format!("reload 后文本为 {:?}", self.buffer.text().as_ref()))
    }
}

impl ExampleState for BufferLifecycleState {
    fn facts(&self) -> Vec<String> {
        let snapshot_stale = self
            .snapshot_version
            .map(|version| self.buffer.is_version_stale(version));
        vec![
            format!("origin = {:?}", self.buffer.origin().handle()),
            format!("version = {:?}", self.buffer.version()),
            format!("saved_version = {:?}", self.buffer.saved_version()),
            format!("dirty = {}", self.buffer.is_dirty()),
            format!("line_count = {}", self.buffer.line_count()),
            format!("snapshot_version = {:?}", self.snapshot_version),
            format!("snapshot stale = {:?}", snapshot_stale),
            format!("snapshot text = {:?}", self.snapshot_text),
        ]
    }

    fn document(&self) -> Option<String> {
        Some(self.buffer.text().to_string())
    }
}

fn err(error: impl ToString) -> String {
    error.to_string()
}
