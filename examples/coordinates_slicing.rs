mod support;

use support::{ExampleAction, ExampleState};
use zom_engine::*;

fn main() {
    let state = CoordinatesSlicingState::new().expect("coordinates slicing example should init");
    support::run_interactive_example(
        "Coordinates / Slicing",
        "点击动作体验 ByteOffset 深核坐标、CharOffset/Position 投影和只读切片。",
        state,
        vec![
            ExampleAction {
                label: "Byte -> Position",
                detail: "把 UTF-8 byte offset 4 转为行列坐标。",
                run: CoordinatesSlicingState::byte_to_position,
            },
            ExampleAction {
                label: "Char -> Byte",
                detail: "把 char offset 2 转回 byte offset。",
                run: CoordinatesSlicingState::char_to_byte,
            },
            ExampleAction {
                label: "Slice Line",
                detail: "读取第 1 行，不修改 Buffer。",
                run: CoordinatesSlicingState::slice_line,
            },
            ExampleAction {
                label: "Slice Viewport",
                detail: "读取两行 viewport，并限制单行字符数。",
                run: CoordinatesSlicingState::slice_viewport,
            },
        ],
    );
}

struct CoordinatesSlicingState {
    buffer: Buffer,
    observations: Vec<String>,
}

impl CoordinatesSlicingState {
    fn new() -> EngineResult<Self> {
        Ok(Self {
            buffer: Buffer::from_text("a你\nemoji 😀".to_string(), BufferConfig::default())?,
            observations: Vec::new(),
        })
    }

    fn byte_to_position(&mut self) -> Result<String, String> {
        let position = self
            .buffer
            .byte_to_position(ByteOffset::new(4))
            .map_err(err)?;
        self.observations
            .push(format!("byte 4 -> position {:?}", position));
        Ok(format!("byte 4 -> position {:?}", position))
    }

    fn char_to_byte(&mut self) -> Result<String, String> {
        let byte = self.buffer.char_to_byte(CharOffset::new(2)).map_err(err)?;
        self.observations.push(format!("char 2 -> byte {}", byte));
        Ok(format!("char 2 -> byte {}", byte))
    }

    fn slice_line(&mut self) -> Result<String, String> {
        let line = self.buffer.slice_line(Line::new(1)).map_err(err)?;
        self.observations
            .push(format!("line[1] = {:?}", line.as_str()));
        Ok(format!("line[1] = {:?}", line.as_str()))
    }

    fn slice_viewport(&mut self) -> Result<String, String> {
        let viewport = self
            .buffer
            .slice_viewport(Viewport::new(Line::ZERO, 2).with_max_line_chars(5))
            .map_err(err)?;
        let message = format!(
            "viewport rows = {}, row[1] truncated = {}",
            viewport.lines().len(),
            viewport.lines()[1].is_truncated()
        );
        self.observations.push(message.clone());
        Ok(message)
    }
}

impl ExampleState for CoordinatesSlicingState {
    fn facts(&self) -> Vec<String> {
        let mut facts = vec![
            format!("version = {:?}", self.buffer.version()),
            format!("len_bytes = {}", self.buffer.len_bytes()),
            format!("len_chars = {}", self.buffer.len_chars()),
            format!("line_count = {}", self.buffer.line_count()),
        ];
        facts.extend(self.observations.iter().rev().take(4).cloned());
        facts
    }

    fn document(&self) -> Option<String> {
        Some(self.buffer.text().to_string())
    }
}

fn err(error: impl ToString) -> String {
    error.to_string()
}
