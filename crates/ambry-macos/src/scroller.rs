//! AppKit's overlay scroll bars over a canvas GPUI draws: the same scrollers NSScrollView
//! shows, appearing while the canvas moves and fading out after.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, msg_send, sel};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSControlSize, NSScroller, NSScrollerPart, NSScrollerStyle,
    NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::native::{NativeView, Target};

/// Seconds a scroller stays after the canvas stops moving, like NSScrollView's.
const LINGER: f64 = 1.2;

/// What the user did with a [`CanvasScroller`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollerEvent {
    /// Dragged the knob to `0.0..=1.0` of the way along.
    Moved(f64),
    /// Clicked the track before (-1) or after (1) the knob.
    Page(i32),
}

/// One overlay scroll bar along the right or bottom edge of the canvas.
pub struct CanvasScroller {
    native: NativeView,
    scroller: Retained<NSScroller>,
    fade: Retained<Target>,
}

impl CanvasScroller {
    /// # Safety
    /// `ns_view` must point to a live `NSView`, and this must run on the main thread.
    pub unsafe fn new(
        ns_view: NonNull<c_void>,
        vertical: bool,
        on_event: impl Fn(ScrollerEvent) + 'static,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let parent: &NSView = unsafe { ns_view.cast().as_ref() };
        let width = NSScroller::scrollerWidthForControlSize_scrollerStyle(
            NSControlSize::Regular,
            NSScrollerStyle::Overlay,
            mtm,
        );
        // A scroller is vertical when it is taller than wide.
        let size = if vertical {
            NSSize::new(width, width * 4.)
        } else {
            NSSize::new(width * 4., width)
        };
        let scroller =
            NSScroller::initWithFrame(NSScroller::alloc(mtm), NSRect::new(NSPoint::ZERO, size));
        scroller.setScrollerStyle(NSScrollerStyle::Overlay);
        scroller.setEnabled(true);
        scroller.setAlphaValue(0.);
        let target = Target::new(mtm, move |sender| {
            let Some(scroller) = sender.downcast_ref::<NSScroller>() else {
                return;
            };
            on_event(match scroller.hitPart() {
                NSScrollerPart::DecrementPage => ScrollerEvent::Page(-1),
                NSScrollerPart::IncrementPage => ScrollerEvent::Page(1),
                _ => ScrollerEvent::Moved(scroller.doubleValue()),
            });
        });
        unsafe {
            scroller.setTarget(Some(&target));
            scroller.setAction(Some(sel!(fire:)));
        }
        let fade = Target::new(mtm, {
            let scroller = scroller.clone();
            move |_| scroller.animator().setAlphaValue(0.)
        });
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(scroller.clone()));
        Some(Self {
            native: NativeView::host(parent, view, target),
            scroller,
            fade,
        })
    }

    pub fn native(&self) -> &NativeView {
        &self.native
    }

    /// The scroller's thickness, in points.
    pub fn width(&self) -> f64 {
        let size = self.scroller.frame().size;
        size.width.min(size.height)
    }

    /// Shows the visible part as `proportion` of the whole, `value` of the way along. A
    /// proportion of 1 or more hides the scroller: there is nothing to scroll.
    pub fn set(&self, proportion: f64, value: f64) {
        let scrollable = proportion < 1.;
        self.scroller.setKnobProportion(proportion.clamp(0.05, 1.));
        self.scroller.setDoubleValue(value.clamp(0., 1.));
        self.scroller.setEnabled(scrollable);
        self.scroller.setHidden(!scrollable);
    }

    /// Shows the scroller now and fades it out once the canvas has been still for a moment.
    pub fn flash(&self) {
        if self.scroller.isHidden() {
            return;
        }
        self.scroller.setAlphaValue(1.);
        let fade: &AnyObject = &self.fade;
        unsafe {
            let _: () = msg_send![
                objc2::class!(NSObject),
                cancelPreviousPerformRequestsWithTarget: fade
            ];
            let _: () = msg_send![
                fade,
                performSelector: sel!(fire:),
                withObject: fade,
                afterDelay: LINGER
            ];
        }
    }
}

impl Drop for CanvasScroller {
    fn drop(&mut self) {
        let fade: &AnyObject = &self.fade;
        unsafe {
            let _: () = msg_send![
                objc2::class!(NSObject),
                cancelPreviousPerformRequestsWithTarget: fade
            ];
        }
    }
}
