//! The blur behind the toolbar: a frosted band of the window's own material that fades out
//! toward the content, like the edge Finder and Safari blur under their toolbars. It only
//! shows; clicks and scrolls go through to the canvas underneath.
//!
//! The band is a sibling of GPUI's view, laid over it in the window's frame view, the way
//! AppKit lays its own titlebar: a backdrop blurs the layers beside and below it, not the
//! contents of the layer it sits in, so as a subview of GPUI's view it would only tint.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBezierPath, NSColor, NSGradient, NSImage, NSImageResizingMode,
    NSResponder, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSVisualEffectView, NSWindowOrderingMode,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};

/// The share of the band that stays fully frosted before the fade starts.
const SOLID: f64 = 0.6;

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

/// The frosted band along the top of the window, behind the toolbar.
pub struct SoftEdge {
    view: Retained<PassThroughEffectView>,
}

impl Drop for SoftEdge {
    fn drop(&mut self) {
        self.view.removeFromSuperview();
    }
}

impl SoftEdge {
    /// Lays a band `height` points tall over the top of `ns_view`, GPUI's view.
    ///
    /// # Safety
    /// `ns_view` must point to a live `NSView` in a window, and this must run on the main
    /// thread.
    pub unsafe fn new(ns_view: NonNull<c_void>, height: f64) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let content: &NSView = unsafe { ns_view.cast().as_ref() };
        let frame_view = unsafe { content.superview() }?;
        let below = content.frame();
        // Pinned to the top of GPUI's view, as wide as it, whatever the window's size.
        let top = if frame_view.isFlipped() {
            below.origin.y
        } else {
            below.origin.y + below.size.height - height
        };
        let frame = NSRect::new(
            NSPoint::new(below.origin.x, top),
            NSSize::new(below.size.width, height),
        );
        let view: Retained<PassThroughEffectView> =
            unsafe { msg_send![PassThroughEffectView::alloc(mtm), initWithFrame: frame] };
        view.setAutoresizingMask(if frame_view.isFlipped() {
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin
        } else {
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin
        });
        view.setMaterial(NSVisualEffectMaterial::Titlebar);
        view.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
        view.setState(NSVisualEffectState::FollowsWindowActiveState);
        view.setMaskImage(Some(&fade(height)));
        frame_view.addSubview_positioned_relativeTo(
            &view,
            NSWindowOrderingMode::Above,
            Some(content),
        );
        Some(Self { view })
    }
}

/// A mask that is opaque along the top and fades to clear toward the content, stretched to
/// the band's width.
fn fade(height: f64) -> Retained<NSImage> {
    let draw = block2::RcBlock::new(move |rect: NSRect| -> Bool {
        // AppKit counts y from the bottom.
        let solid = rect.size.height * SOLID;
        let fading = rect.size.height - solid;
        NSColor::blackColor().setFill();
        NSBezierPath::fillRect(NSRect::new(
            NSPoint::new(0., fading),
            NSSize::new(rect.size.width, solid),
        ));
        if let Some(gradient) = NSGradient::initWithStartingColor_endingColor(
            NSGradient::alloc(),
            &NSColor::clearColor(),
            &NSColor::blackColor(),
        ) {
            gradient.drawInRect_angle(
                NSRect::new(NSPoint::ZERO, NSSize::new(rect.size.width, fading)),
                90.,
            );
        }
        Bool::YES
    });
    let image =
        NSImage::imageWithSize_flipped_drawingHandler(NSSize::new(1., height), false, &draw);
    image.setResizingMode(NSImageResizingMode::Stretch);
    image
}
