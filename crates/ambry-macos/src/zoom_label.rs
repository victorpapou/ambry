//! The zoom as a percentage in a small pill, bottom left. The digits are tabular, each in a
//! cell of its own, and one that changes rolls to the next like the counters the system draws:
//! a push transition inside the cell.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_app_kit::{
    NSClickGestureRecognizer, NSColor, NSFont, NSTextAlignment, NSTextField, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::{CALayer, CAMediaTimingFunction, CATransaction};

use crate::native::{NativeView, Target};

const HEIGHT: f64 = 22.;
const SIDE: f64 = 9.;
const FONT_SIZE: f64 = 11.;
/// The most digits it shows, for "400%".
const DIGITS: usize = 3;
/// How long a digit takes to roll.
const ROLL: f64 = 0.28;

/// A pill with the zoom's percentage.
pub struct ZoomLabel {
    native: NativeView,
    view: Retained<NSView>,
    digits: Vec<Retained<NSTextField>>,
    percent: Retained<NSTextField>,
    /// The width of a digit's cell and the height of a line of text.
    cell: NSSize,
    shown: RefCell<String>,
    dark: Cell<Option<bool>>,
    _target: Retained<Target>,
}

impl ZoomLabel {
    /// Adds the pill to `ns_view`; a click on it calls `on_click`.
    ///
    /// # Safety
    /// `ns_view` must point to a live `NSView`, and this must run on the main thread.
    pub unsafe fn new(ns_view: NonNull<c_void>, on_click: impl Fn() + 'static) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let parent: &NSView = unsafe { ns_view.cast().as_ref() };
        let font = NSFont::monospacedDigitSystemFontOfSize_weight(FONT_SIZE, unsafe {
            objc2_app_kit::NSFontWeightRegular
        });
        let cell_label = |text: &str| {
            let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
            label.setFont(Some(&font));
            label.setTextColor(Some(&NSColor::secondaryLabelColor()));
            label.setAlignment(NSTextAlignment::Center);
            label
        };
        let probe = cell_label("0").fittingSize();
        let cell = NSSize::new(probe.width.ceil(), probe.height.ceil());

        let view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(100., HEIGHT)),
        );
        view.setWantsLayer(true);
        if let Some(layer) = view.layer() {
            layer.setCornerRadius(HEIGHT / 2.);
        }
        let digits: Vec<_> = (0..DIGITS)
            .map(|_| {
                let label = cell_label("");
                // The roll is clipped to the cell.
                label.setWantsLayer(true);
                if let Some(layer) = label.layer() {
                    layer.setMasksToBounds(true);
                }
                view.addSubview(&label);
                label
            })
            .collect();
        let percent = cell_label("%");
        let percent_width = percent.fittingSize().width.ceil();
        percent.setFrameSize(NSSize::new(percent_width, cell.height));
        view.addSubview(&percent);
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
            digits,
            percent,
            cell,
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

    /// Shows `value` percent; `dark` says which appearance the pill's fill is drawn for.
    pub fn set_percent(&self, value: i32, dark: bool) {
        if self.dark.replace(Some(dark)) != Some(dark) {
            if let Some(layer) = self.view.layer() {
                let fill = if dark {
                    NSColor::colorWithWhite_alpha(1., 0.1)
                } else {
                    NSColor::colorWithWhite_alpha(0., 0.06)
                };
                CATransaction::begin();
                CATransaction::setDisableActions(true);
                layer.setBackgroundColor(Some(&fill.CGColor()));
                CATransaction::commit();
            }
        }
        let text = value.max(0).to_string();
        let text: String = text.chars().take(DIGITS).collect();
        let old = self.shown.replace(text.clone());
        if old == text {
            return;
        }
        // The pill is as wide as its digits, so "30%" has no gap in front of it.
        let regrow = old.len() != text.len();
        if regrow {
            self.arrange(text.len());
        }
        let rising = value_of(&text) > value_of(&old);
        for (index, cell) in self.digits.iter().enumerate() {
            let digit = text.get(index..index + 1).unwrap_or("");
            let before = old.get(index..index + 1).unwrap_or("");
            if digit == before {
                continue;
            }
            if !regrow && !before.is_empty() {
                roll(cell, rising);
            }
            cell.setStringValue(&NSString::from_str(digit));
        }
    }

    /// Sizes the pill for `count` digits and puts its cells in a row.
    fn arrange(&self, count: usize) {
        let width = SIDE * 2. + self.cell.width * count as f64 + self.percent.frame().size.width;
        self.view.setFrameSize(NSSize::new(width, HEIGHT));
        let y = ((HEIGHT - self.cell.height) / 2.).round();
        for (index, cell) in self.digits.iter().enumerate() {
            cell.setHidden(index >= count);
            cell.setFrame(NSRect::new(
                NSPoint::new(SIDE + self.cell.width * index as f64, y),
                self.cell,
            ));
        }
        self.percent
            .setFrameOrigin(NSPoint::new(SIDE + self.cell.width * count as f64, y));
    }
}

fn value_of(text: &str) -> i32 {
    text.parse().unwrap_or(0)
}

/// Pushes the cell's next text in from below when the number rises, from above when it falls.
fn roll(cell: &NSTextField, rising: bool) {
    let (Some(layer), Some(class)) = (cell.layer(), AnyClass::get(c"CATransition")) else {
        return;
    };
    let layer: &CALayer = &layer;
    unsafe {
        let transition: Retained<AnyObject> = msg_send![class, animation];
        let _: () = msg_send![&*transition, setType: &*NSString::from_str("push")];
        let subtype = if rising { "fromBottom" } else { "fromTop" };
        let _: () = msg_send![&*transition, setSubtype: &*NSString::from_str(subtype)];
        let _: () = msg_send![&*transition, setDuration: ROLL];
        let timing = CAMediaTimingFunction::functionWithControlPoints(0.2, 0.9, 0.25, 1.);
        let _: () = msg_send![&*transition, setTimingFunction: &*timing];
        let _: () =
            msg_send![layer, addAnimation: &*transition, forKey: &*NSString::from_str("roll")];
    }
}
