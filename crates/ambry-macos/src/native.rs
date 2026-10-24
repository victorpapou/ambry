use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    AllocAnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class,
    msg_send, sel,
};
use objc2_app_kit::{
    NSBezelStyle, NSButton, NSControlBorderShape, NSControlSize, NSImage, NSSegmentSwitchTracking,
    NSSegmentedControl, NSTabPosition, NSTabView, NSTabViewBorderType, NSTabViewDelegate,
    NSTabViewItem, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

struct TargetIvars {
    on_action: Box<dyn Fn(&AnyObject)>,
}

define_class!(
    // Receives the target-action message of one control.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryActionTarget"]
    #[ivars = TargetIvars]
    struct Target;

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSTabViewDelegate for Target {
        #[unsafe(method(tabView:didSelectTabViewItem:))]
        fn did_select(&self, tab_view: &NSTabView, _item: Option<&NSTabViewItem>) {
            (self.ivars().on_action)(tab_view.as_ref());
        }
    }

    impl Target {
        #[unsafe(method(fire:))]
        fn fire(&self, sender: &AnyObject) {
            (self.ivars().on_action)(sender);
        }
    }
);

impl Target {
    fn new(mtm: MainThreadMarker, on_action: impl Fn(&AnyObject) + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars {
            on_action: Box::new(on_action),
        });
        unsafe { msg_send![super(this), init] }
    }
}

struct Hosted {
    view: Retained<NSView>,
    frame: Cell<Option<NSRect>>,
    _target: Retained<Target>,
}

impl Drop for Hosted {
    fn drop(&mut self) {
        self.view.removeFromSuperview();
    }
}

/// An AppKit view living on top of GPUI's content view. GPUI decides where it goes through
/// [`NativeView::set_frame`]; the view leaves the window when the last clone is dropped.
#[derive(Clone)]
pub struct NativeView(Rc<Hosted>);

impl NativeView {
    fn host(parent: &NSView, view: Retained<NSView>, target: Retained<Target>) -> Self {
        view.setHidden(true);
        parent.addSubview(&view);
        Self(Rc::new(Hosted {
            view,
            frame: Cell::new(None),
            _target: target,
        }))
    }

    /// The size AppKit wants for the control, in points.
    pub fn size(&self) -> (f64, f64) {
        let size = self.0.view.frame().size;
        (size.width, size.height)
    }

    /// Places the view at `(x, y)` from the top-left of the window's content, in points.
    pub fn set_frame(&self, x: f64, y: f64, width: f64, height: f64) {
        let view = &self.0.view;
        let Some(parent) = (unsafe { view.superview() }) else {
            return;
        };
        let y = if parent.isFlipped() {
            y
        } else {
            parent.bounds().size.height - y - height
        };
        let frame = NSRect::new(NSPoint::new(x, y), NSSize::new(width, height));
        if self.0.frame.replace(Some(frame)) != Some(frame) {
            view.setFrame(frame);
        }
        view.setHidden(false);
    }
}

/// Icon tabs: a real NSTabView with only its tab strip showing.
///
/// On macOS 26 only the segmented control NSTabView drives gets the light tab look and the
/// glass lens that lifts while you press and drag. NSTabView draws text labels only, so its
/// segments are given SF Symbols and a capsule shape after the fact.
pub struct TabSwitch {
    native: NativeView,
    tabs: Retained<NSTabView>,
}

impl TabSwitch {
    /// Adds a switch with one icon per `(symbol, description)` to `ns_view`.
    /// `on_change` gets the index the user picked.
    ///
    /// # Safety
    /// `ns_view` must point to a live `NSView`, and this must run on the main thread.
    pub unsafe fn new(
        ns_view: NonNull<c_void>,
        icons: &[(&str, &str)],
        item_width: f64,
        on_change: impl Fn(usize) + 'static,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let parent: &NSView = unsafe { ns_view.cast().as_ref() };
        let images = icon_images(icons)?;
        let tabs = icon_tab_view(mtm, icons.len(), &images, item_width);
        let target = Target::new(mtm, move |sender| {
            let Some(tabs) = sender.downcast_ref::<NSTabView>() else {
                return;
            };
            // NSTabView may rebuild its segments on selection; put the icons back.
            decorate(tabs, &images, item_width);
            if let Some(index) = selected_index(tabs) {
                on_change(index);
            }
        });
        tabs.setDelegate(Some(ProtocolObject::from_ref(&*target)));
        let view: Retained<NSView> = Retained::into_super(tabs.clone());
        Some(Self {
            native: NativeView::host(parent, view, target),
            tabs,
        })
    }

    pub fn native(&self) -> &NativeView {
        &self.native
    }

    /// Selects `index` without reporting a change.
    pub fn select(&self, index: usize) {
        if selected_index(&self.tabs) != Some(index) {
            self.tabs.selectTabViewItemAtIndex(index as isize);
        }
    }
}

fn selected_index(tabs: &NSTabView) -> Option<usize> {
    let item = tabs.selectedTabViewItem()?;
    usize::try_from(tabs.indexOfTabViewItem(&item)).ok()
}

/// A borderless NSTabView with `count` empty tabs, decorated with `images` and sized to
/// its tab strip. `item_width` of 0 keeps AppKit's own segment widths.
pub(crate) fn icon_tab_view(
    mtm: MainThreadMarker,
    count: usize,
    images: &[Retained<NSImage>],
    item_width: f64,
) -> Retained<NSTabView> {
    let tabs = NSTabView::initWithFrame(NSTabView::alloc(mtm), NSRect::ZERO);
    tabs.setTabPosition(NSTabPosition::Top);
    tabs.setTabViewBorderType(NSTabViewBorderType::None);
    for i in 0..count {
        let item = unsafe {
            NSTabViewItem::initWithIdentifier(
                NSTabViewItem::alloc(),
                Some(&NSString::from_str(&i.to_string())),
            )
        };
        item.setLabel(&NSString::from_str(""));
        tabs.addTabViewItem(&item);
    }
    tabs.setFrameSize(tabs.minimumSize());
    decorate(&tabs, images, item_width);
    if let Some(control) = segments(&tabs) {
        // Keep the margins NSTabView puts around its strip.
        let frame = control.frame();
        tabs.setFrameSize(NSSize::new(
            frame.size.width + frame.origin.x * 2.,
            frame.size.height + frame.origin.y * 2.,
        ));
    }
    tabs
}

fn segments(tabs: &NSTabView) -> Option<Retained<NSSegmentedControl>> {
    tabs.subviews()
        .iter()
        .find_map(|view| view.downcast::<NSSegmentedControl>().ok())
}

fn decorate(tabs: &NSTabView, images: &[Retained<NSImage>], item_width: f64) {
    let Some(control) = segments(tabs) else {
        return;
    };
    for (i, image) in images.iter().enumerate() {
        let segment = i as isize;
        control.setLabel_forSegment(&NSString::from_str(""), segment);
        control.setImage_forSegment(Some(image), segment);
        if item_width > 0. {
            control.setWidth_forSegment(item_width, segment);
        }
    }
    if has_liquid_glass() {
        control.setBorderShape(NSControlBorderShape::Capsule);
    }
    control.sizeToFit();
}

pub(crate) fn icon_images(icons: &[(&str, &str)]) -> Option<Vec<Retained<NSImage>>> {
    icons
        .iter()
        .map(|(name, description)| symbol(name, description))
        .collect()
}

/// A round Liquid Glass button with an SF Symbol.
pub struct GlassButton {
    native: NativeView,
}

impl GlassButton {
    /// # Safety
    /// `ns_view` must point to a live `NSView`, and this must run on the main thread.
    pub unsafe fn new(
        ns_view: NonNull<c_void>,
        symbol_name: &str,
        tooltip: &str,
        size: f64,
        on_click: impl Fn() + 'static,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let parent: &NSView = unsafe { ns_view.cast().as_ref() };
        let target = Target::new(mtm, move |_| on_click());
        let image = symbol(symbol_name, tooltip)?;
        let button = unsafe {
            NSButton::buttonWithImage_target_action(&image, Some(&target), Some(sel!(fire:)), mtm)
        };
        button.setControlSize(NSControlSize::Large);
        button.setBezelStyle(if has_liquid_glass() {
            NSBezelStyle::Glass
        } else {
            NSBezelStyle::Circular
        });
        button.setToolTip(Some(&NSString::from_str(tooltip)));
        button.setFrameSize(NSSize::new(size, size));
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(button));
        Some(Self {
            native: NativeView::host(parent, view, target),
        })
    }

    pub fn native(&self) -> &NativeView {
        &self.native
    }
}

/// The private `NSTabViewSegmentedControl` (a public `NSSegmentedControl` subclass) gives
/// the light tab look; plain `NSSegmentedControl` stands in where it is missing.
pub(crate) fn tab_segmented_control(
    icons: &[(&str, &str)],
) -> Option<Retained<NSSegmentedControl>> {
    let class = AnyClass::get(c"NSTabViewSegmentedControl")
        .filter(|class| inherits(class, NSSegmentedControl::class()))
        .unwrap_or(NSSegmentedControl::class());
    let allocated: Allocated<NSSegmentedControl> = unsafe { msg_send![class, alloc] };
    let control = NSSegmentedControl::initWithFrame(allocated, NSRect::ZERO);
    control.setSegmentCount(icons.len() as isize);
    control.setTrackingMode(NSSegmentSwitchTracking::SelectOne);
    for (i, (name, description)) in icons.iter().enumerate() {
        let image = symbol(name, description)?;
        control.setImage_forSegment(Some(&image), i as isize);
        control.setToolTip_forSegment(Some(&NSString::from_str(description)), i as isize);
    }
    if has_liquid_glass() {
        control.setBorderShape(NSControlBorderShape::Capsule);
    }
    control.setSelectedSegment(0);
    control.sizeToFit();
    Some(control)
}

fn inherits(class: &AnyClass, ancestor: &AnyClass) -> bool {
    std::iter::successors(Some(class), |class| class.superclass()).any(|class| class == ancestor)
}

fn has_liquid_glass() -> bool {
    AnyClass::get(c"NSGlassEffectView").is_some()
}

fn symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(description)),
    )
}
