mod support;

use support::{ExampleAction, ExampleState};
use zom_engine::*;

fn main() {
    let state = SearchReplaceState::new().expect("search replace example should init");
    support::run_interactive_example(
        "Search / Replace",
        "点击动作体验 literal / regex 搜索结果版本绑定与原子替换。",
        state,
        vec![
            ExampleAction {
                label: "Search Literal",
                detail: "搜索 red，并记录版本绑定的匹配范围。",
                run: SearchReplaceState::search_literal,
            },
            ExampleAction {
                label: "Replace All",
                detail: "把上一次 literal 搜索结果全部替换为 green。",
                run: SearchReplaceState::replace_all_literal,
            },
            ExampleAction {
                label: "Regex Replace",
                detail: "正则搜索 g([a-z]+)n，并替换第一个匹配。",
                run: SearchReplaceState::replace_first_regex,
            },
            ExampleAction {
                label: "Reset",
                detail: "重置文本，方便重复体验搜索替换。",
                run: SearchReplaceState::reset,
            },
        ],
    );
}

struct SearchReplaceState {
    buffer: Buffer,
    literal_ranges: Vec<TextRange>,
    literal_version: Option<BufferVersion>,
    regex_count: usize,
}

impl SearchReplaceState {
    fn new() -> EngineResult<Self> {
        Ok(Self {
            buffer: Buffer::from_text("red blue red".to_string(), BufferConfig::default())?,
            literal_ranges: Vec::new(),
            literal_version: None,
            regex_count: 0,
        })
    }

    fn search_literal(&mut self) -> Result<String, String> {
        let result = self.buffer.search_literal("red").map_err(err)?;
        self.literal_ranges = result.ranges().collect();
        self.literal_version = Some(result.version());
        Ok(format!("找到 {} 个 red", result.len()))
    }

    fn replace_all_literal(&mut self) -> Result<String, String> {
        let result = self.buffer.search_literal("red").map_err(err)?;
        self.buffer
            .replace_all_search_matches(&result, "green")
            .map_err(err)?;
        self.literal_ranges.clear();
        self.literal_version = None;
        Ok(format!(
            "replace all 后文本为 {:?}",
            self.buffer.text().as_ref()
        ))
    }

    fn replace_first_regex(&mut self) -> Result<String, String> {
        let regex = self
            .buffer
            .search_regex(r"g([a-z]+)n", RegexSearchOptions::new())
            .map_err(err)?;
        self.regex_count = regex.len();
        if regex.is_empty() {
            return Ok("没有 regex 匹配可替换".to_string());
        }
        self.buffer
            .replace_regex_match(&regex, 0, "G-$1-N")
            .map_err(err)?;
        Ok(format!(
            "regex replace 后文本为 {:?}",
            self.buffer.text().as_ref()
        ))
    }

    fn reset(&mut self) -> Result<String, String> {
        self.buffer =
            Buffer::from_text("red blue red".to_string(), BufferConfig::default()).map_err(err)?;
        self.literal_ranges.clear();
        self.literal_version = None;
        self.regex_count = 0;
        Ok("已重置为 red blue red".to_string())
    }
}

impl ExampleState for SearchReplaceState {
    fn facts(&self) -> Vec<String> {
        vec![
            format!("version = {:?}", self.buffer.version()),
            format!("literal result version = {:?}", self.literal_version),
            format!("literal ranges = {:?}", self.literal_ranges),
            format!("regex matches = {}", self.regex_count),
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
