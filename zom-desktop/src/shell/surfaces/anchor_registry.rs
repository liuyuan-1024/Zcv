//! Surface 锚点边界注册表。

use std::collections::HashMap;

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, Global, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Pixels, Window, WindowId,
};

/// 每窗口 Surface anchor bounds。
///
/// 它是 Surface 的内部几何缓存，不是业务状态。
/// 召唤者只需要稳定的`ElementId`；
/// 真实 bounds 由 `track_surface_anchor` 在 prepaint 阶段自动记录。
#[derive(Default)]
pub(crate) struct SurfaceAnchorRegistry {
    windows: HashMap<WindowId, HashMap<ElementId, Bounds<Pixels>>>,
}

impl Global for SurfaceAnchorRegistry {}

impl SurfaceAnchorRegistry {
    pub(crate) fn resolve_anchor(&self, window: &Window, id: &ElementId) -> Option<Bounds<Pixels>> {
        self.windows
            .get(&window.window_handle().window_id())
            .and_then(|bounds| bounds.get(id))
            .copied()
    }

    fn record_element(
        &mut self,
        window_id: WindowId,
        id: ElementId,
        bounds: Bounds<Pixels>,
    ) -> bool {
        let element_bounds = self.windows.entry(window_id).or_default();
        if element_bounds.get(&id).is_some_and(|old| *old == bounds) {
            return false;
        }
        element_bounds.insert(id, bounds);
        true
    }
}

/// 给任意元素套上 Surface anchor bounds 采集器。
pub(crate) fn track_surface_anchor(
    id: impl Into<ElementId>,
    child: impl IntoElement,
) -> TrackSurfaceAnchor {
    TrackSurfaceAnchor {
        id: id.into(),
        child: child.into_any_element(),
    }
}

pub(crate) struct TrackSurfaceAnchor {
    id: ElementId,
    child: AnyElement,
}

impl IntoElement for TrackSurfaceAnchor {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TrackSurfaceAnchor {
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
        let changed = cx.default_global::<SurfaceAnchorRegistry>().record_element(
            window_id,
            self.id.clone(),
            bounds,
        );
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
