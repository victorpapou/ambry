//! The blur behind the toolbar: a frosted band of the window's own material that fades out
//! toward the content, like the edge Finder and Safari blur under their toolbars. It only
//! shows; clicks and scrolls go through to the canvas underneath.
//!
//! The band is a sibling of GPUI's view, laid over it in the window's frame view, the way
//! AppKit lays its own titlebar: a backdrop blurs the layers beside and below it, not the
//! contents of the layer it sits in, so as a subview of GPUI's view it would only tint.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBezierPath, NSColor, NSGradient, NSImage, NSImageResizingMode,
    NSResponder, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
    NSVisualEffectView, NSWindowOrderingMode,
};
use objc2_foundation::{NSArray, NSMutableArray, NSNumber, NSPoint, NSRect, NSSize};
use objc2_quartz_core::{CALayer, CATransaction};

/// The share of the band that stays fully frosted before the fade starts.
const SOLID: f64 = 0.6;
/// How much of the content's own color is laid over the frost, so the band blurs what
/// passes under it without turning the lighter grey of the system's material.
const VEIL: f64 = 0.75;

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
    /// The content's color over the frost, fading out with it.
    veil: Retained<CALayer>,
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
        let veil_view =
            NSView::initWithFrame(NSView::alloc(mtm), NSRect::new(NSPoint::ZERO, frame.size));
        veil_view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let veil: Retained<CALayer> =
            unsafe { msg_send![AnyClass::get(c"CAGradientLayer")?, layer] };
        unsafe {
            let _: () = msg_send![&*veil, setStartPoint: NSPoint::new(0.5, 1.)];
            let _: () = msg_send![&*veil, setEndPoint: NSPoint::new(0.5, 0.)];
            let locations = NSArray::from_retained_slice(&[
                NSNumber::new_f64(0.),
                NSNumber::new_f64(SOLID),
                NSNumber::new_f64(1.),
            ]);
            let _: () = msg_send![&*veil, setLocations: &*locations];
        }
        // A layer-hosting view: AppKit keeps the gradient the size of the view.
        veil_view.setLayer(Some(&veil));
        veil_view.setWantsLayer(true);
        view.addSubview(&veil_view);
        frame_view.addSubview_positioned_relativeTo(
            &view,
            NSWindowOrderingMode::Above,
            Some(content),
        );
        Some(Self {
            view,
            veil,
            color: Cell::new(None),
        })
    }

    /// Tints the band with the content's color, `0xRRGGBB`, as the appearance changes.
    pub fn set_color(&self, rgb: u32) {
        if self.color.replace(Some(rgb)) == Some(rgb) {
            return;
        }
        let channel = |shift: u32| f64::from((rgb >> shift) & 0xff) / 255.;
        let color = |alpha: f64| {
            NSColor::colorWithSRGBRed_green_blue_alpha(channel(16), channel(8), channel(0), alpha)
                .CGColor()
        };
        let (veil, clear) = (color(VEIL), color(0.));
        let colors = NSMutableArray::<AnyObject>::new();
        for color in [&veil, &veil, &clear] {
            colors.addObject(crate::tilt::bridged(&**color));
        }
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        unsafe {
            let _: () = msg_send![&*self.veil, setColors: &*colors];
        }
        CATransaction::commit();
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
