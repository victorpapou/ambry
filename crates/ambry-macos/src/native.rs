use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSBezelStyle, NSButton, NSControlBorderShape, NSControlSize, NSFont, NSGlassEffectView,
    NSImage, NSImageScaling, NSImageView, NSSegmentedControl, NSTabPosition, NSTabView,
    NSTabViewBorderType, NSTabViewDelegate, NSTabViewItem, NSTextAlignment, NSTextField, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

pub(crate) struct TargetIvars {
    on_action: Box<dyn Fn(&AnyObject)>,
    /// Asked before a tab gets selected; tabs that act as buttons answer `false`.
    should_select: Option<Box<dyn Fn(&NSTabView, &NSTabViewItem) -> bool>>,
}

define_class!(
    // Receives the target-action message of one control.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryActionTarget"]
    #[ivars = TargetIvars]
    pub(crate) struct Target;

    unsafe impl NSObjectProtocol for Target {}

    unsafe impl NSTabViewDelegate for Target {
        #[unsafe(method(tabView:didSelectTabViewItem:))]
        fn did_select(&self, tab_view: &NSTabView, _item: Option<&NSTabViewItem>) {
            (self.ivars().on_action)(tab_view.as_ref());
        }

        #[unsafe(method(tabView:shouldSelectTabViewItem:))]
        fn should_select(&self, tab_view: &NSTabView, item: Option<&NSTabViewItem>) -> bool {
            match (&self.ivars().should_select, item) {
                (Some(should_select), Some(item)) => should_select(tab_view, item),
                _ => true,
            }
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
    fn with_tabs(
        mtm: MainThreadMarker,
        on_select: impl Fn(&AnyObject) + 'static,
        should_select: impl Fn(&NSTabView, &NSTabViewItem) -> bool + 'static,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars {
            on_action: Box::new(on_select),
            should_select: Some(Box::new(should_select)),
        });
        unsafe { msg_send![super(this), init] }
    }

    pub(crate) fn new(
        mtm: MainThreadMarker,
        on_action: impl Fn(&AnyObject) + 'static,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars {
            on_action: Box::new(on_action),
            should_select: None,
        });
        unsafe { msg_send![super(this), init] }
    }
}

struct Hosted {
    view: Retained<NSView>,
    frame: Cell<Option<NSRect>>,
    /// Hidden by its owner, whatever GPUI lays out.
    hidden: Cell<bool>,
    _target: Option<Retained<Target>>,
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
    pub(crate) fn host(
        parent: &NSView,
        view: Retained<NSView>,
        target: impl Into<Option<Retained<Target>>>,
    ) -> Self {
        view.setHidden(true);
        parent.addSubview(&view);
        Self(Rc::new(Hosted {
            view,
            frame: Cell::new(None),
            hidden: Cell::new(false),
            _target: target.into(),
        }))
    }

    /// Hides or shows the view whatever GPUI lays out, for views GPUI stops rendering.
    pub fn set_hidden(&self, hidden: bool) {
        self.0.hidden.set(hidden);
        self.0
            .view
            .setHidden(hidden || self.0.frame.get().is_none());
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
        view.setHidden(self.0.hidden.get());
    }
}

/// What the user did in a [`TabSwitch`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabEvent {
    /// A tab was selected.
    Select(usize),
    /// An action segment was pressed.
    Action(usize),
}

/// Icon tabs: a real NSTabView with only its tab strip showing.
///
/// On macOS 26 only the segmented control NSTabView drives gets the light tab look and the
/// glass lens that lifts while you press and drag. NSTabView draws text labels only, so its
/// segments are given SF Symbols and a capsule shape after the fact. Action segments after
/// the tabs never stay selected: they report a press and the selection stays where it was.
pub struct TabSwitch {
    native: NativeView,
    tabs: Retained<NSTabView>,
}

impl TabSwitch {
    /// Adds a strip with one icon tab per `tabs` entry followed by one icon button per
    /// `actions` entry, each a `(symbol, description)`, to `ns_view`.
    ///
    /// # Safety
    /// `ns_view` must point to a live `NSView`, and this must run on the main thread.
    pub unsafe fn new(
        ns_view: NonNull<c_void>,
        tabs: &[(&str, &str)],
        actions: &[(&str, &str)],
        item_width: f64,
        on_event: impl Fn(TabEvent) + 'static,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let parent: &NSView = unsafe { ns_view.cast().as_ref() };
        let icons: Vec<_> = tabs.iter().chain(actions).copied().collect();
        let images = icon_images(&icons)?;
        let view = icon_tab_view(mtm, icons.len(), &images, item_width);
        let tab_count = tabs.len();
        let on_event = Rc::new(on_event);
        let on_action = on_event.clone();
        let target = Target::with_tabs(
            mtm,
            move |sender| {
                let Some(tabs) = sender.downcast_ref::<NSTabView>() else {
                    return;
                };
                // NSTabView may rebuild its segments on selection; put the icons back.
                decorate(tabs, &images, item_width);
                if let Some(index) = selected_index(tabs) {
                    on_event(TabEvent::Select(index));
                }
            },
            move |tabs, item| {
                let index = tabs.indexOfTabViewItem(item).max(0) as usize;
                if index < tab_count {
                    return true;
                }
                on_action(TabEvent::Action(index - tab_count));
                false
            },
        );
        view.setDelegate(Some(ProtocolObject::from_ref(&*target)));
        view.setDrawsBackground(false);
        let hosted = on_glass(mtm, &view);
        Some(Self {
            native: NativeView::host(parent, hosted, target),
            tabs: view,
        })
    }

    pub fn native(&self) -> &NativeView {
        &self.native
    }

    /// Selects tab `index` without reporting a change.
    pub fn select(&self, index: usize) {
        if selected_index(&self.tabs) != Some(index) {
            self.tabs.selectTabViewItemAtIndex(index as isize);
        }
    }
}

/// Puts the tab strip on Liquid Glass cut to the strip itself, so the glass and the strip
/// read as one capsule, without the margins NSTabView keeps around it.
fn on_glass(mtm: MainThreadMarker, tabs: &Retained<NSTabView>) -> Retained<NSView> {
    let plain = || Retained::into_super(tabs.clone());
    if !has_liquid_glass() {
        return plain();
    }
    let Some(control) = segments(tabs) else {
        return plain();
    };
    let strip = control.frame();
    let bounds = NSRect::new(NSPoint::new(0., 0.), strip.size);
    tabs.setFrameOrigin(NSPoint::new(-strip.origin.x, -strip.origin.y));
    let holder = NSView::initWithFrame(NSView::alloc(mtm), bounds);
    holder.addSubview(tabs);
    let glass = NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), bounds);
    glass.setCornerRadius(strip.size.height / 2.);
    glass.setContentView(Some(&holder));
    Retained::into_super(glass)
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
    tabs.setControlSize(control_size());
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
    control.setControlSize(control_size());
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

/// The account picture (or initial) on a round Liquid Glass disc.
pub struct GlassAvatar {
    native: NativeView,
}

impl GlassAvatar {
    /// # Safety
    /// `ns_view` must point to a live `NSView`, and this must run on the main thread.
    pub unsafe fn new(
        ns_view: NonNull<c_void>,
        picture: Option<&std::path::Path>,
        initial: &str,
        size: f64,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let parent: &NSView = unsafe { ns_view.cast().as_ref() };
        let bounds = NSRect::new(NSPoint::new(0., 0.), NSSize::new(size, size));
        let image = picture.and_then(|path| {
            NSImage::initWithContentsOfFile(
                NSImage::alloc(),
                &NSString::from_str(&path.to_string_lossy()),
            )
        });
        let content: Retained<NSView> = match image {
            Some(image) => {
                let view = NSImageView::imageViewWithImage(&image, mtm);
                view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
                view.setFrame(bounds);
                view.setWantsLayer(true);
                if let Some(layer) = view.layer() {
                    layer.setCornerRadius(size / 2.);
                    layer.setMasksToBounds(true);
                }
                Retained::into_super(Retained::into_super(view))
            }
            None => {
                let label = NSTextField::labelWithString(&NSString::from_str(initial), mtm);
                label.setFont(Some(&NSFont::systemFontOfSize(15.)));
                label.setAlignment(NSTextAlignment::Center);
                let height = label.fittingSize().height;
                label.setFrame(NSRect::new(
                    NSPoint::new(0., (size - height) / 2.),
                    NSSize::new(size, height),
                ));
                let holder = NSView::initWithFrame(NSView::alloc(mtm), bounds);
                holder.addSubview(&label);
                holder
            }
        };
        let view: Retained<NSView> = if has_liquid_glass() {
            let glass = NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), bounds);
            glass.setCornerRadius(size / 2.);
            glass.setContentView(Some(&content));
            Retained::into_super(glass)
        } else {
            content
        };
        Some(Self {
            native: NativeView::host(parent, view, None),
        })
    }

    pub fn native(&self) -> &NativeView {
        &self.native
    }
}

/// Sets the grey line under the window title, like Finder's item count.
///
/// # Safety
/// `ns_view` must point to a live `NSView`, and this must run on the main thread.
pub unsafe fn set_window_subtitle(ns_view: NonNull<c_void>, subtitle: &str) {
    let view: &NSView = unsafe { ns_view.cast().as_ref() };
    if let Some(window) = view.window() {
        window.setSubtitle(&NSString::from_str(subtitle));
    }
}

/// Extra large is the 36 pt size toolbar controls use; it exists from macOS 26 on.
fn control_size() -> NSControlSize {
    if has_liquid_glass() {
        NSControlSize::ExtraLarge
    } else {
        NSControlSize::Large
    }
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
