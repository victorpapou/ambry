use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBezelStyle, NSButton, NSControlSize, NSGlassEffectContainerView,
    NSGlassEffectView, NSImage, NSSegmentSwitchTracking, NSSegmentedControl, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
};
use objc2_foundation::{NSArray, NSData, NSPoint, NSRect, NSSize, NSString};

use crate::ToolbarEvent;
use crate::layout::*;

const FRAME_ICON: &[u8] = include_bytes!("../assets/frame@3x.png");

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
        let glass = has_liquid_glass();

        let x = INSET + AVATAR + GAP_AFTER_AVATAR;
        let width = SEGMENTS_WIDTH + GAP + HEIGHT;
        let y = if parent.isFlipped() {
            parent.bounds().size.height - INSET - HEIGHT
        } else {
            INSET
        };
        let frame = rect(x, y, width, HEIGHT);

        // On macOS 26 a glass container lets the two glass shapes blend into each other.
        let (root, content): (Retained<NSView>, Retained<NSView>) = if glass {
            let container = NSGlassEffectContainerView::initWithFrame(
                NSGlassEffectContainerView::alloc(mtm),
                frame,
            );
            container.setSpacing(GAP);
            let content = NSView::initWithFrame(NSView::alloc(mtm), rect(0., 0., width, HEIGHT));
            container.setContentView(Some(&content));
            (Retained::into_super(container), content)
        } else {
            let content = NSView::initWithFrame(NSView::alloc(mtm), frame);
            (content.clone(), content)
        };
        let pinned = if parent.isFlipped() {
            NSAutoresizingMaskOptions::ViewMaxXMargin | NSAutoresizingMaskOptions::ViewMinYMargin
        } else {
            NSAutoresizingMaskOptions::ViewMaxXMargin | NSAutoresizingMaskOptions::ViewMaxYMargin
        };
        root.setAutoresizingMask(pinned);

        // Canvas / list switch.
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
        segments.setControlSize(NSControlSize::Large);
        segments.setSelectedSegment(0);
        segments.setToolTip_forSegment(Some(&NSString::from_str("Canvas")), 0);
        segments.setToolTip_forSegment(Some(&NSString::from_str("List")), 1);
        let segments_shape = capsule(rect(0., 0., SEGMENTS_WIDTH, HEIGHT), glass, mtm);
        let fitting = segments.fittingSize();
        segments.setFrame(rect(
            ((SEGMENTS_WIDTH - fitting.width) / 2.).max(0.),
            ((HEIGHT - fitting.height) / 2.).max(0.),
            fitting.width.min(SEGMENTS_WIDTH),
            fitting.height.min(HEIGHT),
        ));
        add_content(&segments_shape, &segments, glass);
        content.addSubview(&segments_shape);

        // Add button.
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
        add.setToolTip(Some(&NSString::from_str("Add files or folders")));
        let add_frame = rect(SEGMENTS_WIDTH + GAP, 0., HEIGHT, HEIGHT);
        if glass {
            add.setBezelStyle(NSBezelStyle::Glass);
            add.setFrame(add_frame);
            content.addSubview(&add);
        } else {
            add.setBordered(false);
            let add_shape = capsule(add_frame, false, mtm);
            add.setFrame(rect(0., 0., HEIGHT, HEIGHT));
            add_content(&add_shape, &add, false);
            content.addSubview(&add_shape);
        }

        parent.addSubview(&root);
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

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}

/// A rounded backing shape: Liquid Glass on macOS 26, a vibrancy material before that.
fn capsule(frame: NSRect, glass: bool, mtm: MainThreadMarker) -> Retained<NSView> {
    if glass {
        let view = NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), frame);
        view.setCornerRadius(frame.size.height / 2.);
        Retained::into_super(view)
    } else {
        let view = NSVisualEffectView::initWithFrame(NSVisualEffectView::alloc(mtm), frame);
        view.setMaterial(NSVisualEffectMaterial::Popover);
        view.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
        view.setState(NSVisualEffectState::Active);
        view.setWantsLayer(true);
        if let Some(layer) = view.layer() {
            layer.setCornerRadius(frame.size.height / 2.);
            layer.setMasksToBounds(true);
        }
        Retained::into_super(view)
    }
}

fn add_content(shape: &NSView, content: &NSView, glass: bool) {
    if glass {
        // NSGlassEffectView sizes its content view to fill it, so wrap the control.
        let mtm = MainThreadMarker::from(shape);
        let wrapper = NSView::initWithFrame(NSView::alloc(mtm), shape.bounds());
        wrapper.addSubview(content);
        let glass: &NSGlassEffectView = unsafe { &*(shape as *const NSView).cast() };
        glass.setContentView(Some(&wrapper));
    } else {
        shape.addSubview(content);
    }
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
