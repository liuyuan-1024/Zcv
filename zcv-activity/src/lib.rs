//! 来源拥有后台任务状态；消费者订阅变化并读取活动。

use std::rc::Rc;

use gpui::{App, SharedString, Subscription};

pub type CancelActivity = Box<dyn Fn(&mut App)>;
pub type ActivityChanged = Rc<dyn Fn(&mut App)>;

/// 读取领域模块持有的活动状态，并订阅变化。
pub trait ActivitySource: 'static {
    fn observe(&self, changed: ActivityChanged, cx: &mut App) -> Subscription;
    fn collect(&self, activities: &mut Vec<BackgroundActivity>, cx: &App);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivityPhase {
    Queued,
    Running,
    Cancelling,
    Confirming,
}

pub struct BackgroundActivity {
    pub name: SharedString,
    pub phase: ActivityPhase,
    pub progress: Option<Box<dyn Fn() -> Option<String>>>,
    pub detail: Option<SharedString>,
    pub cancel: Option<CancelActivity>,
}

impl BackgroundActivity {
    pub fn new(name: impl Into<SharedString>, phase: ActivityPhase) -> Self {
        Self {
            name: name.into(),
            phase,
            progress: None,
            detail: None,
            cancel: None,
        }
    }

    pub fn with_progress(mut self, read: impl Fn() -> Option<String> + 'static) -> Self {
        self.progress = Some(Box::new(read));
        self
    }

    pub fn with_detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn on_cancel(mut self, cancel: impl Fn(&mut App) + 'static) -> Self {
        self.cancel = Some(Box::new(cancel));
        self
    }
}
