//! Pinch to zoom, as the trackpad does it everywhere: AppKit's own magnification gesture
//! on the window's content view, reporting how far the fingers moved and where.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::runtime::NSObjectProtocol;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSMagnificationGestureRecognizer, NSView};

define_class!(
    // Gets each step of the pinch.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryPinch"]
    #[ivars = Box<dyn Fn(f64, f64, f64)>]
    struct Pinch;

    unsafe impl NSObjectProtocol for Pinch {}

    impl Pinch {
        #[unsafe(method(pinch:))]
        fn pinch(&self, recognizer: &NSMagnificationGestureRecognizer) {
            // The recognizer adds up the whole pinch; each step takes what is new.
            let step = recognizer.magnification();
            recognizer.setMagnification(0.);
            let Some(view) = recognizer.view() else {
                return;
            };
            // The fingers lifting is told as a step of 0, for the zoom to settle.
            let ended = matches!(
                recognizer.state(),
                objc2_app_kit::NSGestureRecognizerState::Ended
                    | objc2_app_kit::NSGestureRecognizerState::Cancelled
            );
            let step = if ended { 0. } else { step };
            if step == 0. && !ended {
                return;
            }
            let at = recognizer.locationInView(Some(&view));
            // From the view's top-left, as GPUI counts.
            let y = if view.isFlipped() {
                at.y
            } else {
                view.bounds().size.height - at.y
            };
            (self.ivars())(step, at.x, y);
        }
    }
);

/// A pinch recognizer on a view, kept alive with this value.
pub struct PinchZoom {
    _target: Retained<Pinch>,
    recognizer: Retained<NSMagnificationGestureRecognizer>,
}

impl Drop for PinchZoom {
    fn drop(&mut self) {
        if let Some(view) = self.recognizer.view() {
            view.removeGestureRecognizer(&self.recognizer);
        }
    }
}

impl PinchZoom {
    /// Makes pinching over `ns_view` call `on_zoom(step, x, y)`: `step` is the fraction the
    /// fingers grew by since the last call, `x` and `y` where they are from the view's
    /// top-left, in points.
    ///
    /// # Safety
    /// `ns_view` must point to a live `NSView`, and this must run on the main thread.
    pub unsafe fn new(
        ns_view: NonNull<c_void>,
        on_zoom: impl Fn(f64, f64, f64) + 'static,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let view: &NSView = unsafe { ns_view.cast().as_ref() };
        let target = Pinch::alloc(mtm).set_ivars(Box::new(on_zoom) as Box<dyn Fn(f64, f64, f64)>);
        let target: Retained<Pinch> = unsafe { msg_send![super(target), init] };
        let recognizer = unsafe {
            NSMagnificationGestureRecognizer::initWithTarget_action(
                NSMagnificationGestureRecognizer::alloc(mtm),
                Some(&target),
                Some(sel!(pinch:)),
            )
        };
        view.addGestureRecognizer(&recognizer);
        Some(Self {
            _target: target,
            recognizer,
        })
    }
}
