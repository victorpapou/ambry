use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBezelStyle, NSButton, NSControlSize, NSImage, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use crate::ToolbarEvent;
use crate::glass_switch::GlassSwitch;
use crate::layout::*;

struct TargetIvars {
    on_event: Rc<dyn Fn(ToolbarEvent)>,
}

define_class!(
    // Receives target-action messages from the toolbar controls.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryToolbarTarget"]
    #[ivars = TargetIvars]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    impl Target {
        #[unsafe(method(addClicked:))]
        fn add_clicked(&self, _sender: &AnyObject) {
            (self.ivars().on_event)(ToolbarEvent::Add);
        }
    }
);

impl Target {
    fn new(on_event: Rc<dyn Fn(ToolbarEvent)>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars { on_event });
        unsafe { msg_send![super(this), init] }
    }
}

/// The native toolbar. Dropping it removes the views from the window.
pub struct Toolbar {
    root: Retained<NSView>,
    switch: Retained<GlassSwitch>,
    _target: Retained<Target>,
}

impl Toolbar {
    /// Adds the toolbar on top of `ns_view`, pinned to its bottom-left corner.
    ///
    /// # Safety
    /// `ns_view` must point to a live `NSView`, and this must run on the main thread.
    pub unsafe fn attach(
        ns_view: NonNull<c_void>,
        on_event: impl Fn(ToolbarEvent) + 'static,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let parent: &NSView = unsafe { ns_view.cast().as_ref() };
        let on_event: Rc<dyn Fn(ToolbarEvent)> = Rc::new(on_event);
        let target = Target::new(on_event.clone(), mtm);

        // Canvas / list switch on a glass capsule, with the lens that follows the pointer.
        let switch = GlassSwitch::new(
            mtm,
            &[("number", "Canvas"), ("list.bullet", "List")],
            HEIGHT,
            move |index| on_event(ToolbarEvent::Mode(index)),
        )?;
        let switch_width = switch.frame().size.width;

        // Add button: a glass button on macOS 26, a round push button before that.
        let plus = symbol("plus", "Add files")?;
        let add = unsafe {
            NSButton::buttonWithImage_target_action(
                &plus,
                Some(&target),
                Some(sel!(addClicked:)),
                mtm,
            )
        };
        add.setControlSize(NSControlSize::Large);
        add.setBezelStyle(if has_liquid_glass() {
            NSBezelStyle::Glass
        } else {
            NSBezelStyle::Circular
        });
        add.setToolTip(Some(&NSString::from_str("Add files or folders")));

        let width = switch_width + GAP + HEIGHT;
        let root = NSView::initWithFrame(NSView::alloc(mtm), rect(0., 0., width, HEIGHT));
        pin_bottom_left(parent, &root, INSET + AVATAR + GAP_AFTER_AVATAR, INSET);
        add.setFrame(rect(switch_width + GAP, 0., HEIGHT, HEIGHT));
        root.addSubview(&switch);
        root.addSubview(&add);
        Some(Self {
            root,
            switch,
            _target: target,
        })
    }

    /// Selects the canvas (0) or list (1) item without sending an event.
    pub fn set_mode(&self, index: usize) {
        self.switch.select(index);
    }
}

impl Drop for Toolbar {
    fn drop(&mut self) {
        self.root.removeFromSuperview();
    }
}

fn has_liquid_glass() -> bool {
    AnyClass::get(c"NSGlassEffectView").is_some()
}

/// Adds `view` to `parent` with its bottom-left corner at (`left`, `bottom`) and keeps it
/// there when the window resizes, whether or not `parent` uses flipped coordinates.
fn pin_bottom_left(parent: &NSView, view: &NSView, left: f64, bottom: f64) {
    let size = view.frame().size;
    if parent.isFlipped() {
        let top = parent.bounds().size.height - bottom - size.height;
        view.setFrame(rect(left, top, size.width, size.height));
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMaxXMargin | NSAutoresizingMaskOptions::ViewMinYMargin,
        );
    } else {
        view.setFrame(rect(left, bottom, size.width, size.height));
        view.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewMaxXMargin | NSAutoresizingMaskOptions::ViewMaxYMargin,
        );
    }
    parent.addSubview(view);
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

fn symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(description)),
    )
}
