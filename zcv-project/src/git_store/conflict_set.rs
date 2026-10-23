//! 工作区文档中的 Git 冲突范围。范围随源文本版本解析，供各视图共享。

use std::sync::Arc;

use gpui::{Context, Entity, EventEmitter, Subscription, Task};
use zcv_git::{ConflictRegion, parse_conflict_regions};
use zcv_language::{LanguageBuffer, LanguageBufferEvent};
use zcv_text::{ByteOffset, Snapshot, TextRange};

#[derive(Clone)]
pub struct ConflictSetSnapshot {
    text: Snapshot,
    regions: Arc<[ConflictRegion]>,
}

impl ConflictSetSnapshot {
    pub fn text(&self) -> &Snapshot {
        &self.text
    }

    pub fn regions(&self) -> &[ConflictRegion] {
        &self.regions
    }
}

pub struct ConflictSet {
    source: Entity<LanguageBuffer>,
    snapshot: Option<ConflictSetSnapshot>,
    parse_task: Option<Task<()>>,
    _source_subscription: Subscription,
}

#[derive(Clone, Copy)]
pub struct ConflictSetChanged;

impl EventEmitter<ConflictSetChanged> for ConflictSet {}

impl ConflictSet {
    pub(super) fn new(source: Entity<LanguageBuffer>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&source, |this, _, event, cx| {
            if *event == LanguageBufferEvent::TextChanged {
                this.reparse(cx);
            }
        });
        Self {
            source,
            snapshot: None,
            parse_task: None,
            _source_subscription: subscription,
        }
    }

    pub fn snapshot(&self) -> Option<ConflictSetSnapshot> {
        self.snapshot.clone()
    }

    pub(super) fn reparse(&mut self, cx: &mut Context<Self>) {
        let text = self.source.read(cx).text_snapshot();
        let background = cx.background_executor().clone();
        self.parse_task = Some(cx.spawn(async move |this, cx| {
            let parsed = background
                .spawn(async move {
                    let range = TextRange::new(ByteOffset::ZERO, text.len_bytes())
                        .expect("源文本全文范围必须有效");
                    let content = text.slice_text(range).expect("源文本必须可读");
                    let regions: Arc<[ConflictRegion]> =
                        parse_conflict_regions(content.as_str()).into();
                    ConflictSetSnapshot { text, regions }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.source.read(cx).text_snapshot().version() != parsed.text.version() {
                    return;
                }
                this.snapshot = Some(parsed);
                cx.emit(ConflictSetChanged);
            });
        }));
    }
}
