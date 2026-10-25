//! The soft shade behind the toolbar: the canvas's own color, opaque along the window's top
//! edge and fading out toward the content, so what scrolls under the toolbar sinks into it.
//! No blur, just the gradient of the design:
//!
//! ```css
//! linear-gradient(rgba(255,255,255,0) 0%, rgba(255,255,255,.25) 20%, rgba(255,255,255,.5) 35%,
//!                 rgba(255,255,255,.75) 45%, #FFF 50%)
//! ```
//!
//! turned upside down so the solid half is at the window's top edge. It only shows; clicks
//! and scrolls go through to the canvas.
//!
//! The band is a sibling of GPUI's view, laid over it in the window's frame view.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSColor, NSResponder, NSView, NSWindowOrderingMode,
};
use objc2_foundation::{NSArray, NSMutableArray, NSNumber, NSPoint, NSRect, NSSize};
use objc2_quartz_core::{CALayer, CATransaction};

/// The gradient's stops from the top edge down: where each is, and how opaque the shade is
/// there. The design's stops, mirrored.
const STOPS: [(f64, f64); 6] = [
    (0., 1.),
    (0.5, 1.),
    (0.55, 0.75),
    (0.65, 0.5),
    (0.8, 0.25),
    (1., 0.),
];

define_class!(
    // A layer-hosting view that never takes a click.
    #[unsafe(super(NSView, NSResponder, objc2::runtime::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbrySoftEdge"]
    struct Shade;

    impl Shade {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> *mut NSView {
            std::ptr::null_mut()
        }
    }
);

/// The shade along the top of the window, behind the toolbar.
pub struct SoftEdge {
    view: Retained<Shade>,
    gradient: Retained<CALayer>,
    color: Cell<Option<u32>>,
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
        let view: Retained<Shade> = unsafe { msg_send![Shade::alloc(mtm), initWithFrame: frame] };
        view.setAutoresizingMask(if frame_view.isFlipped() {
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin
        } else {
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMinYMargin
        });
        let gradient: Retained<CALayer> =
            unsafe { msg_send![AnyClass::get(c"CAGradientLayer")?, layer] };
        unsafe {
            // From the top edge down.
            let _: () = msg_send![&*gradient, setStartPoint: NSPoint::new(0.5, 1.)];
            let _: () = msg_send![&*gradient, setEndPoint: NSPoint::new(0.5, 0.)];
            let locations =
                NSArray::from_retained_slice(&STOPS.map(|(at, _)| NSNumber::new_f64(at)));
            let _: () = msg_send![&*gradient, setLocations: &*locations];
        }
        // A layer-hosting view: AppKit keeps the gradient the size of the view.
        view.setLayer(Some(&gradient));
        view.setWantsLayer(true);
        frame_view.addSubview_positioned_relativeTo(
            &view,
            NSWindowOrderingMode::Above,
            Some(content),
        );
        Some(Self {
            view,
            gradient,
            color: Cell::new(None),
        })
    }

    /// Shows or hides the band, for views that have no content under the toolbar.
    pub fn set_hidden(&self, hidden: bool) {
        self.view.setHidden(hidden);
    }

    /// Colors the shade with the content's color, `0xRRGGBB`, as the appearance changes.
    pub fn set_color(&self, rgb: u32) {
        if self.color.replace(Some(rgb)) == Some(rgb) {
            return;
        }
        let channel = |shift: u32| f64::from((rgb >> shift) & 0xff) / 255.;
        let color = |alpha: f64| {
            NSColor::colorWithSRGBRed_green_blue_alpha(channel(16), channel(8), channel(0), alpha)
                .CGColor()
        };
        let colors = NSMutableArray::<AnyObject>::new();
        for color in STOPS.map(|(_, alpha)| color(alpha)) {
            colors.addObject(crate::tilt::bridged(&*color));
        }
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        unsafe {
            let _: () = msg_send![&*self.gradient, setColors: &*colors];
        }
        CATransaction::commit();
    }
}
