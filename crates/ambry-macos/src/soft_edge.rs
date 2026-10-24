//! The soft shade behind the toolbar: the canvas's own color, opaque along the window's top
//! edge and fading out toward the content, so what scrolls under the toolbar sinks into it.
//! No blur, just a gradient. It only shows; clicks and scrolls go through to the canvas.
//!
//! The band is a sibling of GPUI's view, laid over it in the window's frame view.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSColor, NSResponder, NSView, NSWindowOrderingMode,
};
use objc2_foundation::{NSArray, NSMutableArray, NSNumber, NSPoint, NSRect, NSSize};
use objc2_quartz_core::{CALayer, CATransaction};

/// The share of the band that stays fully shaded before the fade starts, and how opaque the
/// shade is there.
const SOLID: f64 = 0.55;
const OPACITY: f64 = 0.92;

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
            let locations = NSArray::from_retained_slice(&[
                NSNumber::new_f64(0.),
                NSNumber::new_f64(SOLID),
                NSNumber::new_f64(1.),
            ]);
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
        let (shade, clear) = (color(OPACITY), color(0.));
        let colors = NSMutableArray::<AnyObject>::new();
        for color in [&shade, &shade, &clear] {
            colors.addObject(crate::tilt::bridged(&**color));
        }
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        unsafe {
            let _: () = msg_send![&*self.gradient, setColors: &*colors];
        }
        CATransaction::commit();
    }
}
