//! A view that leans toward the pointer in 3D, like a card on an Apple TV: under the
//! pointer it grows a little, tilts so the spot under the pointer presses in, catches a
//! highlight where the pointer is, and springs back flat when the pointer leaves.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObjectProtocol};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
};
use objc2_app_kit::{NSColor, NSEvent, NSTrackingArea, NSTrackingAreaOptions, NSView};
use objc2_foundation::{NSMutableArray, NSPoint, NSRect, NSString, NSValue};
use objc2_quartz_core::{
    CALayer, CATransaction, CATransform3D, CATransform3DIdentity, NSValueCATransform3DAdditions,
};

/// Growth under the pointer, the steepest lean in radians, and the viewer's distance in
/// points: nearer looks deeper.
const SCALE: f64 = 1.08;
const LEAN: f64 = 0.24;
const DISTANCE: f64 = 300.;
/// How fast the lean follows the pointer, and how long the spring back takes.
const FOLLOW: f64 = 0.12;
const SETTLE: f64 = 0.45;
/// The shadow's blur at rest and lifted.
const REST_SHADOW: f64 = 6.;
const LIFTED_SHADOW: f64 = 12.;

pub(crate) struct TiltIvars {
    view: Retained<NSView>,
    /// The highlight that follows the pointer, a radial gradient.
    shine: Retained<CALayer>,
}

define_class!(
    // Hears the pointer over the view it tilts.
    #[unsafe(super(objc2::runtime::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryTilt"]
    #[ivars = TiltIvars]
    pub(crate) struct Tilt;

    unsafe impl NSObjectProtocol for Tilt {}

    impl Tilt {
        #[unsafe(method(mouseEntered:))]
        fn entered(&self, event: &NSEvent) {
            let Some(layer) = self.ivars().view.layer() else {
                return;
            };
            self.center(&layer);
            fade(&self.ivars().shine, 1.);
            shadow(&layer, LIFTED_SHADOW);
            self.follow(&layer, event);
        }

        #[unsafe(method(mouseMoved:))]
        fn moved(&self, event: &NSEvent) {
            if let Some(layer) = self.ivars().view.layer() {
                self.follow(&layer, event);
            }
        }

        #[unsafe(method(mouseExited:))]
        fn exited(&self, _event: &NSEvent) {
            let Some(layer) = self.ivars().view.layer() else {
                return;
            };
            fade(&self.ivars().shine, 0.);
            shadow(&layer, REST_SHADOW);
            settle(&layer);
        }
    }
);

impl Tilt {
    /// Makes `view`, which must have a layer, lean toward the pointer. The returned watcher
    /// must be kept alive as long as the view.
    pub(crate) fn attach(mtm: MainThreadMarker, view: &NSView) -> Option<Retained<Self>> {
        let layer = view.layer()?;
        let shine = shine(&layer)?;
        let this = Self::alloc(mtm).set_ivars(TiltIvars {
            view: view.retain(),
            shine,
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        let tracking = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::MouseMoved
                    | NSTrackingAreaOptions::ActiveAlways
                    | NSTrackingAreaOptions::InVisibleRect,
                Some(&this),
                None,
            )
        };
        view.addTrackingArea(&tracking);
        Some(this)
    }

    /// Pins the layer at its middle, so it grows and leans about it. AppKit puts a view's
    /// layer back on its corner whenever it lays the view out, so this runs on every entry.
    fn center(&self, layer: &CALayer) {
        let frame = self.ivars().view.frame();
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        layer.setAnchorPoint(NSPoint::new(0.5, 0.5));
        layer.setPosition(NSPoint::new(
            frame.origin.x + frame.size.width / 2.,
            frame.origin.y + frame.size.height / 2.,
        ));
        CATransaction::commit();
    }

    /// Leans toward the pointer of `event` and moves the highlight under it.
    fn follow(&self, layer: &CALayer, event: &NSEvent) {
        let view = &self.ivars().view;
        let size = view.bounds().size;
        if size.width <= 0. || size.height <= 0. {
            return;
        }
        let at = view.convertPoint_fromView(event.locationInWindow(), None);
        // -1 to 1 across the view, from its left and bottom edges.
        let x = (at.x / size.width * 2. - 1.).clamp(-1., 1.);
        let y = (at.y / size.height * 2. - 1.).clamp(-1., 1.);
        CATransaction::begin();
        CATransaction::setAnimationDuration(FOLLOW);
        layer.setTransform(leaned(x, y));
        let shine = &self.ivars().shine;
        let middle = NSPoint::new((x + 1.) / 2., (y + 1.) / 2.);
        unsafe {
            let _: () = msg_send![&**shine, setStartPoint: middle];
            let _: () = msg_send![
                &**shine,
                setEndPoint: NSPoint::new(middle.x + 0.75, middle.y + 0.75)
            ];
        }
        CATransaction::commit();
    }
}

/// Grown, leaned so the point at (`x`, `y`) presses in, and seen in perspective.
fn leaned(x: f64, y: f64) -> CATransform3D {
    let mut transform = CATransform3D::new_scale(SCALE, SCALE, 1.);
    transform = transform.rotate(-y * LEAN, 1., 0., 0.);
    transform = transform.rotate(x * LEAN, 0., 1., 0.);
    let mut perspective = unsafe { CATransform3DIdentity };
    perspective.m34 = -1. / DISTANCE;
    transform.concat(perspective)
}

/// Springs the layer back flat and to its size, from wherever it is on screen.
fn settle(layer: &CALayer) {
    let identity = unsafe { CATransform3DIdentity };
    let from = presented(layer).map_or_else(|| layer.transform(), |shown| shown.transform());
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    layer.setTransform(identity);
    CATransaction::commit();
    let Some(class) = AnyClass::get(c"CASpringAnimation") else {
        return;
    };
    unsafe {
        let animation: objc2::rc::Allocated<AnyObject> = msg_send![class, alloc];
        let animation: Retained<AnyObject> =
            msg_send![animation, initWithPerceptualDuration: SETTLE, bounce: 0.3f64];
        let key = NSString::from_str("transform");
        let _: () = msg_send![&*animation, setKeyPath: &*key];
        let _: () = msg_send![&*animation, setFromValue: &*NSValue::valueWithCATransform3D(from)];
        let _: () = msg_send![&*animation, setToValue: &*NSValue::valueWithCATransform3D(identity)];
        let settle: f64 = msg_send![&*animation, settlingDuration];
        let _: () = msg_send![&*animation, setDuration: settle];
        let _: () = msg_send![layer, addAnimation: &*animation, forKey: &*key];
    }
}

fn presented(layer: &CALayer) -> Option<Retained<CALayer>> {
    unsafe { msg_send![layer, presentationLayer] }
}

fn shadow(layer: &CALayer, radius: f64) {
    CATransaction::begin();
    CATransaction::setAnimationDuration(0.25);
    layer.setShadowRadius(radius);
    CATransaction::commit();
}

fn fade(layer: &CALayer, opacity: f32) {
    CATransaction::begin();
    CATransaction::setAnimationDuration(0.2);
    layer.setOpacity(opacity);
    CATransaction::commit();
}

/// A soft white spot over `layer`, hidden until the pointer comes.
fn shine(layer: &CALayer) -> Option<Retained<CALayer>> {
    let class = AnyClass::get(c"CAGradientLayer")?;
    let bright = NSColor::colorWithWhite_alpha(1., 0.32).CGColor();
    let clear = NSColor::colorWithWhite_alpha(1., 0.).CGColor();
    let colors = NSMutableArray::<AnyObject>::new();
    for color in [&bright, &clear] {
        // CGColor is toll-free bridged to an Objective-C object.
        let object: &AnyObject = unsafe { &*((&**color) as *const _ as *const AnyObject) };
        colors.addObject(object);
    }
    unsafe {
        let shine: Retained<CALayer> = msg_send![class, layer];
        let _: () = msg_send![&*shine, setType: &*NSString::from_str("radial")];
        let _: () = msg_send![&*shine, setColors: &*colors];
        shine.setFrame(layer.bounds());
        shine.setCornerRadius(layer.cornerRadius());
        shine.setMasksToBounds(true);
        shine.setOpacity(0.);
        layer.addSublayer(&shine);
        Some(shine)
    }
}
