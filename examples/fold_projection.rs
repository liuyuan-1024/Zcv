mod support;

use support::{ExampleAction, ExampleState};
use zom_engine::*;

fn main() {
    let state = FoldProjectionState::new().expect("fold projection example should init");
    support::run_interactive_example(
        "Fold / Projection",
        "点击动作体验 FoldSet、Projection 行映射和折叠后 viewport 切片。",
        state,
        vec![
            ExampleAction {
                label: "Fold Lines 1..3",
                detail: "折叠 two / three 两行，并构建投影。",
                run: FoldProjectionState::fold_lines,
            },
            ExampleAction {
                label: "Slice Projection",
                detail: "读取折叠后的 projected viewport。",
                run: FoldProjectionState::slice_projection,
            },
            ExampleAction {
                label: "Insert Above",
                detail: "在顶部插入一行，并用 DeltaEvent 推进 folds。",
                run: FoldProjectionState::insert_above,
            },
            ExampleAction {
                label: "Unfold All",
                detail: "清空 FoldSet，恢复完整逻辑文本。",
                run: FoldProjectionState::unfold_all,
            },
        ],
    );
}

struct FoldProjectionState {
    buffer: Buffer,
    folds: FoldSet,
    projected_lines: usize,
    placeholders: usize,
    line_two_hidden: Option<bool>,
}

impl FoldProjectionState {
    fn new() -> EngineResult<Self> {
        let buffer =
            Buffer::from_text("one\ntwo\nthree\nfour".to_string(), BufferConfig::default())?;
        let folds = FoldSet::new(buffer.version());
        Ok(Self {
            buffer,
            folds,
            projected_lines: 4,
            placeholders: 0,
            line_two_hidden: None,
        })
    }

    fn fold_lines(&mut self) -> Result<String, String> {
        self.folds.unfold_all();
        self.folds
            .fold_lines(
                &self.buffer,
                LineRange::new(Line::new(1), Line::new(3)).map_err(err)?,
            )
            .map_err(err)?;
        self.refresh_projection()?;
        Ok(format!("folds = {}", self.folds.len()))
    }

    fn slice_projection(&mut self) -> Result<String, String> {
        self.refresh_projection()?;
        Ok(format!(
            "projected_lines = {}, placeholders = {}",
            self.projected_lines, self.placeholders
        ))
    }

    fn insert_above(&mut self) -> Result<String, String> {
        self.buffer
            .insert(ByteOffset::ZERO, "zero\n")
            .map_err(err)?;
        let event = self
            .buffer
            .last_delta_event()
            .ok_or_else(|| "缺少 DeltaEvent".to_string())?
            .clone();
        self.folds.update_through_delta_event(&event).map_err(err)?;
        self.refresh_projection()?;
        Ok("插入文本并推进 FoldSet 版本".to_string())
    }

    fn unfold_all(&mut self) -> Result<String, String> {
        self.folds.unfold_all();
        self.refresh_projection()?;
        Ok("已清空 folds".to_string())
    }

    fn refresh_projection(&mut self) -> Result<(), String> {
        let snapshot = self.buffer.snapshot();
        let projection = Projection::build(&snapshot, &self.folds).map_err(err)?;
        let slice = projection
            .slice_viewport(
                &snapshot,
                ProjectedViewport::new(ProjectedLineIndex::ZERO, 6),
            )
            .map_err(err)?;
        self.projected_lines = projection.line_count();
        self.placeholders = slice.placeholders().len();
        self.line_two_hidden = Some(
            projection
                .is_logical_line_hidden(Line::new(2))
                .map_err(err)?,
        );
        Ok(())
    }
}

impl ExampleState for FoldProjectionState {
    fn facts(&self) -> Vec<String> {
        vec![
            format!("buffer version = {:?}", self.buffer.version()),
            format!("fold version = {:?}", self.folds.version()),
            format!("fold count = {}", self.folds.len()),
            format!("logical line count = {}", self.buffer.line_count()),
            format!("projected line count = {}", self.projected_lines),
            format!("line 2 hidden = {:?}", self.line_two_hidden),
            format!("placeholders = {}", self.placeholders),
        ]
    }

    fn document(&self) -> Option<String> {
        Some(self.buffer.text().to_string())
    }
}

fn err(error: impl ToString) -> String {
    error.to_string()
}
