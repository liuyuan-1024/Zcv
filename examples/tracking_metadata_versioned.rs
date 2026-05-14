mod support;

use support::{ExampleAction, ExampleState};
use zom_engine::*;

fn main() {
    let state = TrackingMetadataState::new().expect("tracking metadata example should init");
    support::run_interactive_example(
        "Tracking / Metadata / Versioned",
        "点击动作体验 Anchor/TrackedRange 推进、MetadataLayer 跟随和 VersionedResult 版本绑定。",
        state,
        vec![
            ExampleAction {
                label: "Attach Metadata",
                detail: "给 beta 绑定一段 metadata range。",
                run: TrackingMetadataState::attach_metadata,
            },
            ExampleAction {
                label: "Insert Prefix",
                detail: "在文本开头插入 >>，并用 DeltaEvent 推进 layer。",
                run: TrackingMetadataState::insert_prefix,
            },
            ExampleAction {
                label: "Versioned Result",
                detail: "创建绑定当前 BufferVersion 的 VersionedResult。",
                run: TrackingMetadataState::versioned_result,
            },
            ExampleAction {
                label: "Reset",
                detail: "重置文本和 metadata layer，方便重复体验。",
                run: TrackingMetadataState::reset,
            },
        ],
    );
}

struct TrackingMetadataState {
    buffer: Buffer,
    layer: MetadataLayer<&'static str>,
    id: Option<MetadataRangeId>,
    versioned_stale: Option<bool>,
}

impl TrackingMetadataState {
    fn new() -> EngineResult<Self> {
        let buffer = Buffer::from_text("alpha beta".to_string(), BufferConfig::default())?;
        let layer =
            MetadataLayer::with_kind(MetadataLayerKind::custom("example"), buffer.version());
        Ok(Self {
            buffer,
            layer,
            id: None,
            versioned_stale: None,
        })
    }

    fn attach_metadata(&mut self) -> Result<String, String> {
        let id = self
            .layer
            .insert(
                TextRange::new(ByteOffset::new(6), ByteOffset::new(10)).map_err(err)?,
                "payload",
            )
            .map_err(err)?;
        self.id = Some(id);
        Ok(format!("metadata id = {:?}", id))
    }

    fn insert_prefix(&mut self) -> Result<String, String> {
        self.buffer.insert(ByteOffset::ZERO, ">> ").map_err(err)?;
        let event = self
            .buffer
            .last_delta_event()
            .ok_or_else(|| "缺少 DeltaEvent".to_string())?
            .clone();
        self.layer.update_through_delta_event(&event).map_err(err)?;
        Ok(format!("推进后 layer version = {:?}", self.layer.version()))
    }

    fn versioned_result(&mut self) -> Result<String, String> {
        let result = VersionedResult::new(self.buffer.version(), ByteOffset::new(0));
        self.versioned_stale = Some(result.is_stale(self.buffer.version()));
        Ok(format!(
            "versioned payload stale = {:?}",
            self.versioned_stale
        ))
    }

    fn reset(&mut self) -> Result<String, String> {
        let fresh = Self::new().map_err(err)?;
        *self = fresh;
        Ok("已重置 tracking 场景".to_string())
    }
}

impl ExampleState for TrackingMetadataState {
    fn facts(&self) -> Vec<String> {
        let tracked = self
            .id
            .and_then(|id| self.layer.get(id))
            .map(|range| range.range());
        vec![
            format!("buffer version = {:?}", self.buffer.version()),
            format!("layer version = {:?}", self.layer.version()),
            format!("metadata id = {:?}", self.id),
            format!("tracked range = {:?}", tracked),
            format!("versioned stale = {:?}", self.versioned_stale),
        ]
    }

    fn document(&self) -> Option<String> {
        Some(self.buffer.text().to_string())
    }
}

fn err(error: impl ToString) -> String {
    error.to_string()
}
