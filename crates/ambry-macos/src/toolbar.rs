use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBezelStyle, NSButton, NSColor, NSControlBorderShape,
    NSControlSize, NSImage, NSSegmentDistribution, NSSegmentSwitchTracking, NSSegmentedControl,
    NSView,
};
use objc2_foundation::{NSArray, NSData, NSPoint, NSRect, NSSize, NSString};

use crate::ToolbarEvent;
use crate::layout::*;

const FRAME_ICON: &[u8] = include_bytes!("../assets/frame@3x.png");
const SEGMENT_WIDTH: f64 = 32.;

struct TargetIvars {
    on_event: Box<dyn Fn(ToolbarEvent)>,
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
        #[unsafe(method(segmentChanged:))]
        fn segment_changed(&self, sender: &NSSegmentedControl) {
            let index = sender.selectedSegment().max(0) as usize;
            (self.ivars().on_event)(ToolbarEvent::Mode(index));
        }

        #[unsafe(method(addClicked:))]
        fn add_clicked(&self, _sender: &AnyObject) {
            (self.ivars().on_event)(ToolbarEvent::Add);
        }
    }
);

impl Target {
    fn new(on_event: Box<dyn Fn(ToolbarEvent)>, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars { on_event });
        unsafe { msg_send![super(this), init] }
    }
}

/// The native toolbar. Dropping it removes the views from the window.
pub struct Toolbar {
    root: Retained<NSView>,
    segments: Retained<NSSegmentedControl>,
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
        let target = Target::new(Box::new(on_event), mtm);

        // Canvas / list switch: a plain NSSegmentedControl, drawn by AppKit as is.
        let images = NSArray::from_retained_slice(&[
            template_png(FRAME_ICON, 18.)?,
            symbol("list.bullet", "List")?,
        ]);
        let segments = unsafe {
            NSSegmentedControl::segmentedControlWithImages_trackingMode_target_action(
                &images,
                NSSegmentSwitchTracking::SelectOne,
                Some(&target),
                Some(sel!(segmentChanged:)),
                mtm,
            )
        };
        // Tab-style switch like Xcode's inspector. On macOS 26 AppKit draws the capsule
        // selection and the glass lens itself, so only older systems get a custom colour.
        segments.setControlSize(NSControlSize::Large);
        segments.setSegmentDistribution(NSSegmentDistribution::FillEqually);
        for segment in 0..2 {
            segments.setWidth_forSegment(SEGMENT_WIDTH, segment);
        }
        if has_liquid_glass() {
            segments.setBorderShape(NSControlBorderShape::Capsule);
        } else {
            segments.setSelectedSegmentBezelColor(Some(
                &NSColor::unemphasizedSelectedContentBackgroundColor(),
            ));
        }
        segments.setSelectedSegment(0);
        segments.setToolTip_forSegment(Some(&NSString::from_str("Canvas")), 0);
        segments.setToolTip_forSegment(Some(&NSString::from_str("List")), 1);
        let segments_size = segments.fittingSize();

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

        let width = segments_size.width + GAP + HEIGHT;
        let root = NSView::initWithFrame(NSView::alloc(mtm), rect(0., 0., width, HEIGHT));
        pin_bottom_left(parent, &root, INSET + AVATAR + GAP_AFTER_AVATAR, INSET);
        segments.setFrame(rect(
            0.,
            ((HEIGHT - segments_size.height) / 2.).max(0.),
            segments_size.width,
            segments_size.height.min(HEIGHT),
        ));
        add.setFrame(rect(segments_size.width + GAP, 0., HEIGHT, HEIGHT));
        root.addSubview(&segments);
        root.addSubview(&add);
        Some(Self {
            root,
            segments,
            _target: target,
        })
    }

    /// Selects the canvas (0) or list (1) segment without sending an event.
    pub fn set_mode(&self, index: usize) {
        self.segments.setSelectedSegment(index as isize);
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

fn template_png(bytes: &[u8], points: f64) -> Option<Retained<NSImage>> {
    let data = NSData::with_bytes(bytes);
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    image.setSize(NSSize::new(points, points));
    image.setTemplate(true);
    Some(image)
}
