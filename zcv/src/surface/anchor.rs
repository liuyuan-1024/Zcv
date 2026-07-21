//! Surface 锚点边界注册表。
//!
//! 按钮等"召唤者"用 `track_anchor` 包裹，prepaint 阶段自动记录渲染位置。
//! Surface 打开时读取注册表算出锚点坐标，传给 `Anchored` 定位。

use std::collections::HashMap;

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, Global, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Pixels, Window, WindowId,
};

/// 每窗口 Surface anchor bounds。
///
/// 由 `track_anchor` 在 prepaint 阶段自动写入。
#[derive(Default)]
pub(crate) struct AnchorRegistry {
    windows: HashMap<WindowId, HashMap<ElementId, Bounds<Pixels>>>,
}

impl Global for AnchorRegistry {}

impl AnchorRegistry {
    /// 解析锚点元素在当前窗口中的渲染边界。
    pub fn resolve(&self, window: &Window, id: &ElementId) -> Option<Bounds<Pixels>> {
        self.windows
            .get(&window.window_handle().window_id())
            .and_then(|bounds| bounds.get(id))
            .copied()
    }

    fn record(&mut self, window_id: WindowId, id: ElementId, bounds: Bounds<Pixels>) -> bool {
        let window_bounds = self.windows.entry(window_id).or_default();
        let changed = window_bounds
            .get(&id)
            .map(|old| *old != bounds)
            .unwrap_or(true);
        window_bounds.insert(id, bounds);
        changed
    }
}

/// 包裹一个元素，自动采集它的渲染边界到全局 `AnchorRegistry`。
pub(crate) fn track_anchor(id: impl Into<ElementId>, child: impl IntoElement) -> TrackAnchor {
    TrackAnchor {
        id: id.into(),
        child: child.into_any_element(),
    }
}

pub(crate) struct TrackAnchor {
    id: ElementId,
    child: AnyElement,
}

impl IntoElement for TrackAnchor {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TrackAnchor {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let window_id = window.window_handle().window_id();
        let changed =
            cx.default_global::<AnchorRegistry>()
                .record(window_id, self.id.clone(), bounds);
        if changed {
            window.refresh();
        }
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}
