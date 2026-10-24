//! A GPUI element that hosts a native AppKit view, so the view takes part in GPUI layout.
//!
//! The element sizes itself to the control's own size, or fills its parent, and every frame
//! moves the view to wherever layout put the element.

use ambry_macos::NativeView;
use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, Style, Window, px, relative, size,
};

pub struct Native {
    view: NativeView,
    fill: bool,
}

/// Lays out `view` like any other element.
pub fn native(view: &NativeView) -> Native {
    Native {
        view: view.clone(),
        fill: false,
    }
}

impl Native {
    /// Takes the parent's whole size instead of the view's own.
    pub fn fill(mut self) -> Self {
        self.fill = true;
        self
    }
}

impl IntoElement for Native {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Native {
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
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        if self.fill {
            style.size = size(relative(1.).into(), relative(1.).into());
        } else {
            let (width, height) = self.view.size();
            style.size = size(px(width as f32).into(), px(height as f32).into());
            style.flex_shrink = 0.;
        }
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
        self.view.set_frame(
            f32::from(bounds.origin.x).into(),
            f32::from(bounds.origin.y).into(),
            f32::from(bounds.size.width).into(),
            f32::from(bounds.size.height).into(),
        );
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}
