use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBezelStyle, NSButton, NSColor, NSControlSize, NSGlassEffectView,
    NSImage, NSSegmentStyle, NSSegmentSwitchTracking, NSSegmentedControl, NSTextField, NSView,
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
        segments.setControlSize(NSControlSize::Large);
        // Neutral selection like toolbar switches, instead of the accent-blue default.
        segments.setSelectedSegmentBezelColor(Some(&NSColor::quaternaryLabelColor()));
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

    /// Temporary: stacks numbered variants of the canvas/list switch above the toolbar
    /// so the design can be picked from one screenshot.
    ///
    /// # Safety
    /// Same as [`Toolbar::attach`].
    pub unsafe fn attach_switch_gallery(ns_view: NonNull<c_void>) -> Option<()> {
        let mtm = MainThreadMarker::new()?;
        let parent: &NSView = unsafe { ns_view.cast().as_ref() };
        let glass = has_liquid_glass();
        let variants: [(bool, NSSegmentStyle, Option<Retained<NSColor>>); 8] = [
            (false, NSSegmentStyle::Automatic, None),
            (false, NSSegmentStyle::Separated, None),
            (false, NSSegmentStyle::Capsule, None),
            (false, NSSegmentStyle::TexturedRounded, None),
            (glass, NSSegmentStyle::Automatic, None),
            (glass, NSSegmentStyle::Separated, None),
            (glass, NSSegmentStyle::Capsule, None),
            (
                glass,
                NSSegmentStyle::Automatic,
                Some(NSColor::tertiarySystemFillColor()),
            ),
        ];
        // Rows live in one panel pinned like the toolbar, so they follow window resizes.
        let row = HEIGHT + 14.;
        let panel = NSView::initWithFrame(NSView::alloc(mtm), rect(0., 0., 320., row * 8.));
        pin_bottom_left(parent, &panel, INSET, INSET + row);
        for (index, (in_glass, style, bezel)) in variants.into_iter().enumerate() {
            let images = NSArray::from_retained_slice(&[
                template_png(FRAME_ICON, 18.)?,
                symbol("list.bullet", "List")?,
            ]);
            let segments = unsafe {
                NSSegmentedControl::segmentedControlWithImages_trackingMode_target_action(
                    &images,
                    NSSegmentSwitchTracking::SelectOne,
                    None,
                    None,
                    mtm,
                )
            };
            segments.setControlSize(NSControlSize::Large);
            segments.setSegmentStyle(style);
            if let Some(bezel) = &bezel {
                segments.setSelectedSegmentBezelColor(Some(bezel));
            }
            segments.setSelectedSegment(0);
            let size = segments.fittingSize();

            let y = row * index as f64;
            let label =
                NSTextField::labelWithString(&NSString::from_str(&format!("{}", index + 1)), mtm);
            label.setFrame(rect(0., y + (HEIGHT - 17.) / 2., 24., 17.));
            panel.addSubview(&label);

            let x = AVATAR + GAP_AFTER_AVATAR;
            if in_glass {
                let pad = 3.;
                let frame = rect(x, y, size.width + 2. * pad, HEIGHT);
                let shape = NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), frame);
                shape.setCornerRadius(HEIGHT / 2.);
                let wrapper = NSView::initWithFrame(
                    NSView::alloc(mtm),
                    rect(0., 0., frame.size.width, HEIGHT),
                );
                segments.setFrame(rect(
                    pad,
                    ((HEIGHT - size.height) / 2.).max(0.),
                    size.width,
                    size.height.min(HEIGHT),
                ));
                wrapper.addSubview(&segments);
                shape.setContentView(Some(&wrapper));
                panel.addSubview(&shape);
            } else {
                segments.setFrame(rect(
                    x,
                    y + ((HEIGHT - size.height) / 2.).max(0.),
                    size.width,
                    size.height.min(HEIGHT),
                ));
                panel.addSubview(&segments);
            }
        }
        Some(())
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
