//! Overlay anchor bounds registry。
//!
//! GPUI 目前没有给 shell 暴露“按 ElementId 查询 bounds”的直接 API，所以由
//! anchor provider 在 prepaint 阶段登记自己的窗口坐标，OverlayShell 再按
//! `OverlayAnchor` 解析到具体位置。

use std::collections::HashMap;

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window,
};

use super::OverlayAnchor;

/// 每窗口 overlay anchor bounds。由 `ShellView` 创建为 GPUI Entity。
#[derive(Default)]
pub(crate) struct AnchorRegistry {
    element_bounds: HashMap<ElementId, Bounds<Pixels>>,
}

impl AnchorRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn resolve(&self, anchor: &OverlayAnchor) -> Option<Bounds<Pixels>> {
        match anchor {
            OverlayAnchor::Element(id) => self.element_bounds.get(id).copied(),
        }
    }

    fn record_element(&mut self, id: ElementId, bounds: Bounds<Pixels>) -> bool {
        if self
            .element_bounds
            .get(&id)
            .is_some_and(|old| *old == bounds)
        {
            return false;
        }
        self.element_bounds.insert(id, bounds);
        true
    }
}

/// 给任意元素套上 anchor bounds 采集器。
pub(crate) fn track_anchor(
    id: impl Into<ElementId>,
    registry: gpui::Entity<AnchorRegistry>,
    child: impl IntoElement,
) -> TrackAnchor {
    TrackAnchor {
        id: id.into(),
        registry,
        child: child.into_any_element(),
    }
}

pub(crate) struct TrackAnchor {
    id: ElementId,
    registry: gpui::Entity<AnchorRegistry>,
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
        self.registry.update(cx, |registry, cx| {
            if registry.record_element(self.id.clone(), bounds) {
                cx.notify();
            }
        });
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
