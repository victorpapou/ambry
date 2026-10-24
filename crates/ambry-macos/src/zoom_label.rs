//! The zoom as a percentage, bottom left: plain grey text in tabular digits, no backdrop. When
//! the number changes it rolls to the next like the counters the system draws, a push
//! transition up when it grows and down when it shrinks.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadMarker, MainThreadOnly, msg_send, sel};
use objc2_app_kit::{NSClickGestureRecognizer, NSColor, NSFont, NSTextField, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::{CALayer, CAMediaTimingFunction};

use crate::native::{NativeView, Target};

const HEIGHT: f64 = 22.;
const FONT_SIZE: f64 = 11.;
/// How long the number takes to roll.
const ROLL: f64 = 0.28;

/// The zoom's percentage.
pub struct ZoomLabel {
    native: NativeView,
    view: Retained<NSView>,
    label: Retained<NSTextField>,
    shown: Cell<Option<i32>>,
    _target: Retained<Target>,
}

impl ZoomLabel {
    /// Adds the label to `ns_view`; a click on it calls `on_click`.
    ///
    /// # Safety
    /// `ns_view` must point to a live `NSView`, and this must run on the main thread.
    pub unsafe fn new(ns_view: NonNull<c_void>, on_click: impl Fn() + 'static) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let parent: &NSView = unsafe { ns_view.cast().as_ref() };
        let label = NSTextField::labelWithString(&NSString::from_str("100%"), mtm);
        label.setFont(Some(&NSFont::monospacedDigitSystemFontOfSize_weight(
            FONT_SIZE,
            unsafe { objc2_app_kit::NSFontWeightRegular },
        )));
        label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        // The roll is clipped to the label.
        label.setWantsLayer(true);
        if let Some(layer) = label.layer() {
            layer.setMasksToBounds(true);
        }
        let view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(40., HEIGHT)),
        );
        view.addSubview(&label);
        view.setToolTip(Some(&NSString::from_str("Actual Size")));
        let target = Target::new(mtm, move |_| on_click());
        let click = unsafe {
            NSClickGestureRecognizer::initWithTarget_action(
                NSClickGestureRecognizer::alloc(mtm),
                Some(&target),
                Some(sel!(fire:)),
            )
        };
        view.addGestureRecognizer(&click);
        let this = Self {
            native: NativeView::host(parent, view.clone(), None),
            view,
            label,
            shown: Cell::new(None),
            _target: target,
        };
        this.set_percent(100, false);
        Some(this)
    }

    pub fn native(&self) -> &NativeView {
        &self.native
    }

    /// Shows `value` percent. The label's grey follows the appearance by itself, so `_dark`
    /// is unused.
    pub fn set_percent(&self, value: i32, _dark: bool) {
        let old = self.shown.replace(Some(value));
        if old == Some(value) {
            return;
        }
        if let Some(old) = old {
            roll(&self.label, value > old);
        }
        self.label
            .setStringValue(&NSString::from_str(&format!("{value}%")));
        let size = self.label.fittingSize();
        let width = size.width.ceil();
        self.label.setFrame(NSRect::new(
            NSPoint::new(0., ((HEIGHT - size.height) / 2.).round()),
            NSSize::new(width, size.height.ceil()),
        ));
        self.view.setFrameSize(NSSize::new(width, HEIGHT));
    }
}

/// Pushes the label's next text in from below when the number rises, from above when it falls.
fn roll(label: &NSTextField, rising: bool) {
    let (Some(layer), Some(class)) = (label.layer(), AnyClass::get(c"CATransition")) else {
        return;
    };
    let layer: &CALayer = &layer;
    unsafe {
        let transition: Retained<AnyObject> = msg_send![class, animation];
        let _: () = msg_send![&*transition, setType: &*NSString::from_str("push")];
        let subtype = if rising { "fromTop" } else { "fromBottom" };
        let _: () = msg_send![&*transition, setSubtype: &*NSString::from_str(subtype)];
        let _: () = msg_send![&*transition, setDuration: ROLL];
        let timing = CAMediaTimingFunction::functionWithControlPoints(0.2, 0.9, 0.25, 1.);
        let _: () = msg_send![&*transition, setTimingFunction: &*timing];
        let _: () =
            msg_send![layer, addAnimation: &*transition, forKey: &*NSString::from_str("roll")];
    }
}
