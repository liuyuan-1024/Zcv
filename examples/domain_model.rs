mod support;

use support::{ExampleAction, ExampleState};
use zom_engine::*;

fn main() {
    support::run_interactive_example(
        "Domain Model",
        "点击动作体验强类型坐标、范围不变量和宿主不透明来源句柄。",
        DomainModelState::default(),
        vec![
            ExampleAction {
                label: "Build Range",
                detail: "构造合法 TextRange [2, 5)，观察长度和 contains 语义。",
                run: DomainModelState::build_range,
            },
            ExampleAction {
                label: "Reject Range",
                detail: "尝试构造 start > end 的 TextRange，观察可恢复错误。",
                run: DomainModelState::reject_range,
            },
            ExampleAction {
                label: "Switch Origin",
                detail: "切换宿主来源句柄，不暴露宿主内部对象。",
                run: DomainModelState::switch_origin,
            },
        ],
    );
}

struct DomainModelState {
    range: Option<TextRange>,
    origin: BufferOrigin,
    last_error: Option<String>,
}

impl Default for DomainModelState {
    fn default() -> Self {
        Self {
            range: None,
            origin: BufferOrigin::external("memory://example/domain"),
            last_error: None,
        }
    }
}

impl DomainModelState {
    fn build_range(&mut self) -> Result<String, String> {
        let range = TextRange::new(ByteOffset::new(2), ByteOffset::new(5)).map_err(err)?;
        self.range = Some(range);
        self.last_error = None;
        Ok(format!("构造 TextRange {:?}，len = {}", range, range.len()))
    }

    fn reject_range(&mut self) -> Result<String, String> {
        match TextRange::new(ByteOffset::new(5), ByteOffset::new(2)) {
            Ok(range) => {
                self.range = Some(range);
                Ok("意外构造成功".to_string())
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
                Ok(format!("非法 range 被拒绝: {}", error))
            }
        }
    }

    fn switch_origin(&mut self) -> Result<String, String> {
        self.origin = BufferOrigin::external("memory://example/alternate");
        Ok(format!("origin handle = {:?}", self.origin.handle()))
    }
}

impl ExampleState for DomainModelState {
    fn facts(&self) -> Vec<String> {
        vec![
            format!("range = {:?}", self.range),
            format!(
                "range contains byte 2 = {:?}",
                self.range.map(|r| r.contains(ByteOffset::new(2)))
            ),
            format!(
                "range contains byte 5 = {:?}",
                self.range.map(|r| r.contains(ByteOffset::new(5)))
            ),
            format!("origin kind = {:?}", self.origin.kind()),
            format!("origin handle = {:?}", self.origin.handle()),
            format!("last error = {:?}", self.last_error),
        ]
    }
}

fn err(error: impl ToString) -> String {
    error.to_string()
}
