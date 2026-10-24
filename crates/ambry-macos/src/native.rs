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
    NSAnimatablePropertyContainer, NSBezelStyle, NSButton, NSColor, NSControlBorderShape,
    NSControlSize, NSFont, NSGlassEffectView, NSImage, NSImageScaling, NSImageView,
    NSSegmentedControl, NSTabPosition, NSTabView, NSTabViewBorderType, NSTabViewDelegate,
    NSTabViewItem, NSTextAlignment, NSTextField, NSView,
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
    pub(crate) fn with_tabs(
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
    glass: Option<Retained<NSGlassEffectView>>,
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
        let hosted = cut_to_strip(mtm, &view);
        Some(Self {
            native: NativeView::host(parent, hosted, target),
            tabs: view,
            glass: None,
        })
    }

    pub fn native(&self) -> &NativeView {
        &self.native
    }

    /// Tints the glass under the strip with `(white, alpha)` and fades the strip itself, to
    /// match the buttons beside it.
    pub fn set_look(&self, tint: Option<(f64, f64)>, strip_alpha: f64) {
        if let Some(glass) = &self.glass {
            glass.setTintColor(tint_color(tint).as_deref());
        }
        self.tabs.setAlphaValue(strip_alpha);
    }

    /// The views behind the strip, by name, for [`crate::probe::run`].
    pub fn probe_targets(&self) -> Vec<(String, Retained<AnyObject>)> {
        let mut targets = vec![("tabs".to_string(), object(self.tabs.clone()))];
        if let Some(glass) = &self.glass {
            targets.push(("tabs-root".into(), object(glass.clone())));
        }
        if let Some(control) = segments(&self.tabs) {
            if let Some(cell) = control.cell() {
                targets.push(("cell".into(), object(cell)));
            }
            if let Some(hosting) = control.subviews().firstObject() {
                targets.push(("hosting".into(), object(hosting)));
            }
            targets.push(("control".into(), object(control)));
        }
        targets
    }

    /// Selects tab `index` without reporting a change.
    pub fn select(&self, index: usize) {
        if selected_index(&self.tabs) != Some(index) {
            self.tabs.selectTabViewItemAtIndex(index as isize);
        }
    }
}

/// A view exactly the size of the tab strip with the tab view inside it, so the window lays
/// out the strip itself and not NSTabView's margins. The strip keeps the system look,
/// press bubble included.
fn cut_to_strip(mtm: MainThreadMarker, tabs: &Retained<NSTabView>) -> Retained<NSView> {
    let Some(control) = segments(tabs) else {
        return Retained::into_super(tabs.clone());
    };
    let strip = control.frame();
    tabs.setFrameOrigin(NSPoint::new(-strip.origin.x, -strip.origin.y));
    let holder = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0., 0.), strip.size),
    );
    holder.addSubview(tabs);
    holder
}

pub(crate) fn object<T: objc2::Message>(object: Retained<T>) -> Retained<AnyObject> {
    // SAFETY: every Objective-C object is an AnyObject.
    unsafe { Retained::cast_unchecked(object) }
}

fn tint_color(tint: Option<(f64, f64)>) -> Option<Retained<NSColor>> {
    tint.map(|(white, alpha)| NSColor::colorWithWhite_alpha(white, alpha))
}

/// A native capsule segmented control with icon segments and a grey selection pill.
pub struct GlassSegments {
    native: NativeView,
    control: Retained<NSSegmentedControl>,
}

impl GlassSegments {
    /// Adds one icon segment per `(symbol, description)` to `ns_view`; `on_select` gets the
    /// segment the user picks.
    ///
    /// # Safety
    /// `ns_view` must point to a live `NSView`, and this must run on the main thread.
    pub unsafe fn new(
        ns_view: NonNull<c_void>,
        icons: &[(&str, &str)],
        item_width: f64,
        on_select: impl Fn(usize) + 'static,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let parent: &NSView = unsafe { ns_view.cast().as_ref() };
        let images = icon_images(icons)?;
        let target = Target::new(mtm, move |sender| {
            if let Some(control) = sender.downcast_ref::<NSSegmentedControl>() {
                if let Ok(index) = usize::try_from(control.selectedSegment()) {
                    on_select(index);
                }
            }
        });
        let control = unsafe {
            NSSegmentedControl::segmentedControlWithImages_trackingMode_target_action(
                &objc2_foundation::NSArray::from_retained_slice(&images),
                objc2_app_kit::NSSegmentSwitchTracking::SelectOne,
                Some(&target),
                Some(sel!(fire:)),
                mtm,
            )
        };
        control.setControlSize(control_size());
        if has_liquid_glass() {
            control.setBorderShape(NSControlBorderShape::Capsule);
        }
        for segment in 0..icons.len() as isize {
            if item_width > 0. {
                control.setWidth_forSegment(item_width, segment);
            }
        }
        control.setSelectedSegmentBezelColor(Some(&NSColor::systemFillColor()));
        control.setSelectedSegment(0);
        control.sizeToFit();
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(control.clone()));
        Some(Self {
            native: NativeView::host(parent, view, target),
            control,
        })
    }

    pub fn native(&self) -> &NativeView {
        &self.native
    }

    /// Selects segment `index` without reporting it.
    pub fn select(&self, index: usize) {
        if self.control.selectedSegment() != index as isize {
            self.control.setSelectedSegment(index as isize);
        }
    }

    /// Fades the segments.
    pub fn set_alpha(&self, alpha: f64) {
        self.control.setAlphaValue(alpha);
    }

    /// The views behind the segments, by name, for [`crate::probe::run`].
    pub fn probe_targets(&self) -> Vec<(String, Retained<AnyObject>)> {
        let mut targets = vec![("control".to_string(), object(self.control.clone()))];
        if let Some(hosting) = self.control.subviews().firstObject() {
            targets.push(("hosting".into(), object(hosting)));
        }
        targets
    }
}

pub(crate) fn selected_index(tabs: &NSTabView) -> Option<usize> {
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

pub(crate) fn segments(tabs: &NSTabView) -> Option<Retained<NSSegmentedControl>> {
    tabs.subviews()
        .iter()
        .find_map(|view| view.downcast::<NSSegmentedControl>().ok())
}

pub(crate) fn decorate(tabs: &NSTabView, images: &[Retained<NSImage>], item_width: f64) {
    if let Some(control) = segments(tabs) {
        decorate_strip(&control, images, item_width);
    }
}

/// Puts `images` on a tab strip's segments, `item_width` points wide (0 keeps AppKit's).
pub(crate) fn decorate_strip(
    control: &NSSegmentedControl,
    images: &[Retained<NSImage>],
    item_width: f64,
) {
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
    button: Retained<NSButton>,
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
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(button.clone()));
        Some(Self {
            native: NativeView::host(parent, view, target),
            button,
        })
    }

    pub fn native(&self) -> &NativeView {
        &self.native
    }

    /// Tints the glass with `(white, alpha)`.
    pub fn set_tint(&self, tint: Option<(f64, f64)>) {
        self.button.setBezelColor(tint_color(tint).as_deref());
    }

    /// The button and its insides, by name, for [`crate::probe::run`].
    pub fn probe_targets(&self) -> Vec<(String, Retained<AnyObject>)> {
        let mut targets = vec![("button-root".to_string(), object(self.button.clone()))];
        if let Some(cell) = self.button.cell() {
            targets.push(("button-cell".into(), object(cell)));
        }
        if let Some(hosting) = self.button.subviews().firstObject() {
            targets.push(("button-hosting".into(), object(hosting)));
        }
        targets
    }
}

/// Icon tabs on one Liquid Glass capsule, the same glass as [`GlassAvatar`], with a grey pill
/// behind the selected tab that slides when the selection changes.
pub struct GlassTabs {
    native: NativeView,
    glass: Option<Retained<NSGlassEffectView>>,
    pill: Retained<NSView>,
    buttons: Vec<Retained<NSButton>>,
    item_width: f64,
    selected: Cell<usize>,
    pill_tint: Cell<(f64, f64)>,
}

/// Room between the capsule's edge and the selection pill.
const PILL_INSET: f64 = 2.;

impl GlassTabs {
    /// One tab `item_width` points wide per `(symbol, description)`, on a capsule `height`
    /// points tall. `on_select` gets the tab the user picks.
    ///
    /// # Safety
    /// `ns_view` must point to a live `NSView`, and this must run on the main thread.
    pub unsafe fn new(
        ns_view: NonNull<c_void>,
        icons: &[(&str, &str)],
        height: f64,
        item_width: f64,
        on_select: impl Fn(usize) + 'static,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let parent: &NSView = unsafe { ns_view.cast().as_ref() };
        let target = Target::new(mtm, move |sender| {
            if let Some(button) = sender.downcast_ref::<NSButton>() {
                on_select(button.tag().max(0) as usize);
            }
        });
        let width = icons.len() as f64 * item_width + PILL_INSET * 2.;
        let bounds = NSRect::new(NSPoint::new(0., 0.), NSSize::new(width, height));
        let content = NSView::initWithFrame(NSView::alloc(mtm), bounds);

        let pill = NSView::initWithFrame(NSView::alloc(mtm), NSRect::ZERO);
        pill.setWantsLayer(true);
        if let Some(layer) = pill.layer() {
            layer.setCornerRadius((height - PILL_INSET * 2.) / 2.);
        }
        content.addSubview(&pill);

        let mut buttons = Vec::new();
        for (index, (name, description)) in icons.iter().enumerate() {
            let image = symbol(name, description)?;
            let button = unsafe {
                NSButton::buttonWithImage_target_action(
                    &image,
                    Some(&target),
                    Some(sel!(fire:)),
                    mtm,
                )
            };
            button.setBordered(false);
            button.setToolTip(Some(&NSString::from_str(description)));
            button.setTag(index as isize);
            button.setFrame(NSRect::new(
                NSPoint::new(PILL_INSET + index as f64 * item_width, 0.),
                NSSize::new(item_width, height),
            ));
            content.addSubview(&button);
            buttons.push(button);
        }

        let glass = has_liquid_glass().then(|| {
            let glass = NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), bounds);
            glass.setCornerRadius(height / 2.);
            glass.setContentView(Some(&content));
            glass
        });
        let view: Retained<NSView> = match &glass {
            Some(glass) => Retained::into_super(glass.clone()),
            None => content,
        };
        let tabs = Self {
            native: NativeView::host(parent, view, target),
            glass,
            pill,
            buttons,
            item_width,
            selected: Cell::new(0),
            pill_tint: Cell::new((0., 0.08)),
        };
        tabs.place_pill(false);
        tabs.paint();
        Some(tabs)
    }

    pub fn native(&self) -> &NativeView {
        &self.native
    }

    /// Moves the pill to tab `index`.
    pub fn select(&self, index: usize) {
        if self.selected.replace(index) != index {
            self.place_pill(true);
        }
    }

    /// The pill's color as `(white, alpha)`, set per appearance by the caller.
    pub fn set_pill_tint(&self, tint: (f64, f64)) {
        self.pill_tint.set(tint);
        self.paint();
    }

    /// Tints the capsule's glass with `(white, alpha)`.
    pub fn set_glass_tint(&self, tint: Option<(f64, f64)>) {
        if let Some(glass) = &self.glass {
            glass.setTintColor(tint_color(tint).as_deref());
        }
    }

    fn place_pill(&self, animated: bool) {
        let Some(button) = self.buttons.first() else {
            return;
        };
        let height = button.frame().size.height - PILL_INSET * 2.;
        let frame = NSRect::new(
            NSPoint::new(
                PILL_INSET + self.selected.get() as f64 * self.item_width,
                PILL_INSET,
            ),
            NSSize::new(self.item_width, height),
        );
        if animated {
            self.pill.animator().setFrame(frame);
        } else {
            self.pill.setFrame(frame);
        }
    }

    fn paint(&self) {
        let (white, alpha) = self.pill_tint.get();
        if let Some(layer) = self.pill.layer() {
            let color = NSColor::colorWithWhite_alpha(white, alpha).CGColor();
            layer.setBackgroundColor(Some(&color));
        }
    }

    /// The glass, the pill and the buttons, by name, for [`crate::probe::run`].
    pub fn probe_targets(&self) -> Vec<(String, Retained<AnyObject>)> {
        let mut targets = vec![("pill".to_string(), object(self.pill.clone()))];
        if let Some(glass) = &self.glass {
            targets.push(("tabs-root".to_string(), object(glass.clone())));
        }
        for (index, button) in self.buttons.iter().enumerate() {
            targets.push((format!("b{index}"), object(button.clone())));
        }
        targets
    }
}

/// The account picture (or initial) on a round Liquid Glass disc.
pub struct GlassAvatar {
    native: NativeView,
    glass: Option<Retained<NSGlassEffectView>>,
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
        let glass = has_liquid_glass().then(|| {
            let glass = NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), bounds);
            glass.setCornerRadius(size / 2.);
            glass.setContentView(Some(&content));
            glass
        });
        let view: Retained<NSView> = match &glass {
            Some(glass) => Retained::into_super(glass.clone()),
            None => content,
        };
        Some(Self {
            native: NativeView::host(parent, view, None),
            glass,
        })
    }

    pub fn native(&self) -> &NativeView {
        &self.native
    }

    /// Tints the glass with `(white, alpha)`.
    pub fn set_tint(&self, tint: Option<(f64, f64)>) {
        if let Some(glass) = &self.glass {
            glass.setTintColor(tint_color(tint).as_deref());
        }
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
pub(crate) fn control_size() -> NSControlSize {
    if has_liquid_glass() {
        NSControlSize::Large
    } else {
        NSControlSize::Regular
    }
}

fn has_liquid_glass() -> bool {
    AnyClass::get(c"NSGlassEffectView").is_some()
}

pub(crate) fn symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(description)),
    )
}
