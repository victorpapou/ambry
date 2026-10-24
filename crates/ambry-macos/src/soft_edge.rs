//! The soft backing under the toolbar and the footer: a frosted band of the window's own
//! material that fades out toward the content, like the edge Finder and Safari blur under
//! their toolbars. It only shows; clicks and scrolls go through to the canvas underneath.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBezierPath, NSColor, NSGradient, NSImage, NSImageResizingMode, NSResponder, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::native::NativeView;

/// The share of the band that stays fully frosted before the fade starts.
const SOLID: f64 = 0.45;

define_class!(
    // A visual effect view that never takes a click.
    #[unsafe(super(NSVisualEffectView, NSView, NSResponder, objc2::runtime::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbrySoftEdge"]
    struct PassThroughEffectView;

    impl PassThroughEffectView {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }
    }
);

/// Which edge of the window the band sits on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
}

/// A frosted band along one edge of the window.
pub struct SoftEdge {
    native: NativeView,
}

impl SoftEdge {
    /// # Safety
    /// `ns_view` must point to a live `NSView`, and this must run on the main thread.
    pub unsafe fn new(ns_view: NonNull<c_void>, edge: Edge, height: f64) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let parent: &NSView = unsafe { ns_view.cast().as_ref() };
        let frame = NSRect::new(NSPoint::ZERO, NSSize::new(100., height));
        let view: Retained<PassThroughEffectView> =
            unsafe { msg_send![PassThroughEffectView::alloc(mtm), initWithFrame: frame] };
        view.setMaterial(NSVisualEffectMaterial::HeaderView);
        view.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
        view.setState(NSVisualEffectState::FollowsWindowActiveState);
        view.setMaskImage(Some(&fade(edge, height)));
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(view));
        Some(Self {
            native: NativeView::host(parent, view, None),
        })
    }

    pub fn native(&self) -> &NativeView {
        &self.native
    }
}

/// A mask that is opaque along `edge` and fades to clear away from it, stretched to the
/// band's width.
fn fade(edge: Edge, height: f64) -> Retained<NSImage> {
    let draw = block2::RcBlock::new(move |rect: NSRect| -> Bool {
        let solid = rect.size.height * SOLID;
        let fading = rect.size.height - solid;
        // AppKit counts y from the bottom.
        let (solid_y, fading_y, angle) = match edge {
            Edge::Top => (rect.size.height - solid, 0., 90.),
            Edge::Bottom => (0., solid, 270.),
        };
        NSColor::blackColor().setFill();
        NSBezierPath::fillRect(NSRect::new(
            NSPoint::new(0., solid_y),
            NSSize::new(rect.size.width, solid),
        ));
        if let Some(gradient) = NSGradient::initWithStartingColor_endingColor(
            NSGradient::alloc(),
            &NSColor::clearColor(),
            &NSColor::blackColor(),
        ) {
            gradient.drawInRect_angle(
                NSRect::new(
                    NSPoint::new(0., fading_y),
                    NSSize::new(rect.size.width, fading),
                ),
                angle,
            );
        }
        Bool::YES
    });
    let image =
        NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(1., height), false, &draw);
    image.setResizingMode(NSImageResizingMode::Stretch);
    image
}
