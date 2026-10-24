//! The zoom as a percentage, bottom left: plain grey text in tabular digits, no backdrop. Each
//! digit sits in a slot of its own, one digit's advance wide, so the number reads at the font's
//! own spacing; a digit that changes rolls to the next like the counters the system draws, up
//! when the number grows and down when it shrinks, while the others stay put.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadMarker, MainThreadOnly, msg_send, sel};
use objc2_app_kit::{NSClickGestureRecognizer, NSColor, NSFont, NSView};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::{CALayer, CAMediaTimingFunction, CATransaction};

use crate::native::{NativeView, Target};

const HEIGHT: f64 = 22.;
const FONT_SIZE: f64 = 11.;
/// The most digits it shows, for "400".
const DIGITS: usize = 3;
/// How long a digit takes to roll.
const ROLL: f64 = 0.28;

/// The zoom's percentage.
pub struct ZoomLabel {
    native: NativeView,
    view: Retained<NSView>,
    /// A clipping slot per digit, each holding one text layer, then the "%".
    slots: Vec<Retained<CALayer>>,
    texts: Vec<Retained<CALayer>>,
    percent: Retained<CALayer>,
    font: Retained<NSFont>,
    /// A digit's advance, the "%"'s width, and a line's height.
    digit: f64,
    sign: f64,
    line: f64,
    shown: RefCell<String>,
    dark: Cell<Option<bool>>,
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
        let font = NSFont::monospacedDigitSystemFontOfSize_weight(FONT_SIZE, unsafe {
            objc2_app_kit::NSFontWeightRegular
        });
        let measure = |text: &str| -> NSSize {
            let key = unsafe { objc2_app_kit::NSFontAttributeName };
            let attributes = NSDictionary::from_slices(&[key], &[&*font as &AnyObject]);
            unsafe { msg_send![&*NSString::from_str(text), sizeWithAttributes: &*attributes] }
        };
        let digit = measure("0").width;
        let percent_size = measure("%");
        let line = percent_size.height.ceil();

        let view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(40., HEIGHT)),
        );
        view.setWantsLayer(true);
        let root = view.layer()?;
        let scale = parent
            .window()
            .map_or(2., |window| window.backingScaleFactor());
        let text_layer = |text: &str| -> Option<Retained<CALayer>> {
            let layer: Retained<CALayer> =
                unsafe { msg_send![AnyClass::get(c"CATextLayer")?, layer] };
            unsafe {
                let _: () = msg_send![&*layer, setFont: &*font];
                let _: () = msg_send![&*layer, setFontSize: FONT_SIZE];
                let _: () = msg_send![&*layer, setString: &*NSString::from_str(text)];
                let _: () = msg_send![&*layer, setAlignmentMode: &*NSString::from_str("left")];
            }
            layer.setContentsScale(scale);
            Some(layer)
        };
        let mut slots = Vec::new();
        let mut texts = Vec::new();
        for _ in 0..DIGITS {
            let slot = CALayer::new();
            slot.setMasksToBounds(true);
            let text = text_layer("")?;
            slot.addSublayer(&text);
            root.addSublayer(&slot);
            slots.push(slot);
            texts.push(text);
        }
        let percent = text_layer("%")?;
        root.addSublayer(&percent);

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
            slots,
            texts,
            percent,
            font,
            digit,
            sign: percent_size.width.ceil(),
            line,
            shown: RefCell::new(String::new()),
            dark: Cell::new(None),
            _target: target,
        };
        this.set_percent(100, false);
        Some(this)
    }

    pub fn native(&self) -> &NativeView {
        &self.native
    }

    /// Shows `value` percent; `dark` says which appearance its grey is drawn for.
    pub fn set_percent(&self, value: i32, dark: bool) {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        if self.dark.replace(Some(dark)) != Some(dark) {
            let grey = if dark {
                NSColor::colorWithWhite_alpha(1., 0.55)
            } else {
                NSColor::colorWithWhite_alpha(0., 0.5)
            };
            for layer in self.texts.iter().chain([&self.percent]) {
                unsafe {
                    let _: () = msg_send![&**layer, setForegroundColor: &*grey.CGColor()];
                }
            }
        }
        let text: String = value.max(0).to_string().chars().take(DIGITS).collect();
        let old = self.shown.replace(text.clone());
        if old != text {
            let regrow = old.len() != text.len();
            if regrow {
                self.arrange(text.len());
            }
            let rising = text.parse::<i32>().unwrap_or(0) > old.parse::<i32>().unwrap_or(0);
            for (index, layer) in self.texts.iter().enumerate() {
                let digit = text.get(index..index + 1).unwrap_or("");
                let before = old.get(index..index + 1).unwrap_or("");
                if digit == before && !regrow {
                    continue;
                }
                if !regrow && !before.is_empty() {
                    roll(&self.slots[index], rising);
                }
                unsafe {
                    let _: () = msg_send![&**layer, setString: &*NSString::from_str(digit)];
                }
            }
        }
        CATransaction::commit();
        let _ = &self.font;
    }

    /// Lays out `count` digit slots and the "%" after them.
    fn arrange(&self, count: usize) {
        let width = (self.digit * count as f64 + self.sign).ceil();
        self.view.setFrameSize(NSSize::new(width, HEIGHT));
        let y = ((HEIGHT - self.line) / 2.).round();
        for (index, (slot, text)) in self.slots.iter().zip(&self.texts).enumerate() {
            slot.setHidden(index >= count);
            slot.setFrame(NSRect::new(
                NSPoint::new(self.digit * index as f64, y),
                NSSize::new(self.digit, self.line),
            ));
            text.setFrame(NSRect::new(
                NSPoint::ZERO,
                NSSize::new(self.digit + 2., self.line),
            ));
        }
        self.percent.setFrame(NSRect::new(
            NSPoint::new(self.digit * count as f64, y),
            NSSize::new(self.sign + 2., self.line),
        ));
    }
}

/// Pushes the slot's next digit in from below when the number rises, from above when it falls.
fn roll(slot: &CALayer, rising: bool) {
    let Some(class) = AnyClass::get(c"CATransition") else {
        return;
    };
    unsafe {
        let transition: Retained<AnyObject> = msg_send![class, animation];
        let _: () = msg_send![&*transition, setType: &*NSString::from_str("push")];
        // A view's layer counts y up from the bottom: "from bottom" comes up from below.
        let subtype = if rising { "fromBottom" } else { "fromTop" };
        let _: () = msg_send![&*transition, setSubtype: &*NSString::from_str(subtype)];
        let _: () = msg_send![&*transition, setDuration: ROLL];
        let timing = CAMediaTimingFunction::functionWithControlPoints(0.2, 0.9, 0.25, 1.);
        let _: () = msg_send![&*transition, setTimingFunction: &*timing];
        let _: () =
            msg_send![slot, addAnimation: &*transition, forKey: &*NSString::from_str("roll")];
    }
}
