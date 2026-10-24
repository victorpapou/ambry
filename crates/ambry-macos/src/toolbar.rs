//! The window's own NSToolbar: the box's icon and name on the left, and the add button, the
//! canvas/list switch and the account on the right. macOS draws them with the toolbar's glass,
//! grey selection and press bubble.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::path::Path;
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::{Duration, Instant};

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSBitmapImageRep, NSButton,
    NSClickGestureRecognizer, NSColor, NSCompositingOperation, NSControlSize,
    NSDeviceRGBColorSpace, NSEvent, NSFont, NSGraphicsContext, NSImage, NSImageScaling,
    NSImageSymbolConfiguration, NSImageSymbolScale, NSImageView, NSLayoutConstraint, NSMenu,
    NSMenuItem, NSPopoverDelegate, NSSegmentedControl, NSStackView, NSTabView, NSTextField,
    NSToolbar, NSToolbarDelegate, NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem,
    NSToolbarItemIdentifier, NSToolbarSpaceItemIdentifier, NSTrackingArea, NSTrackingAreaOptions,
    NSUserInterfaceLayoutOrientation, NSView, NSWindowToolbarStyle, NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSCopying, NSMutableArray, NSNotification, NSNumber, NSPoint, NSRect, NSRectEdge,
    NSSize, NSString, NSURL,
};
use objc2_quartz_core::{CALayer, CAMediaTimingFunction, CATransaction, CATransform3DIdentity};

use crate::box_list::BoxList;
use crate::icon_picker::IconPicker;
use crate::native::{
    Target, decorate_strip, icon_images, icon_tab_view, segments, selected_index, symbol,
};
use crate::settings::{self, Pane};

const BOX: &str = "box";
const MODES: &str = "modes";
const AVATAR: &str = "avatar";
/// Width given to each canvas/list segment; AppKit pads it to about Freeform's 37 and 39.
const MODE_WIDTH: f64 = 32.;
/// Point size of the account picture inside its glass circle.
const AVATAR_SIZE: f64 = 34.;
/// The box's icon tile next to its name, as big as a shortcut's in the Shortcuts editor.
const TILE: f64 = 22.;
/// Font size of the box's name and status.
const TITLE_SIZE: f64 = 14.;
/// One pass of the light sweeping over the status, in seconds, and its lean from upright.
const SWEEP: f64 = 1.6;
const SWEEP_LEAN: f64 = 30.;
const CHEVRON_SIZE: f64 = 12.;
/// Room the chevron takes after the name when it shows.
const CHEVRON_GAP: f64 = 3.;
/// How far under the account button its menu opens.
const MENU_GAP: f64 = 10.;
const CHEVRON_DURATION: f64 = 0.2;
/// How soon after the box list closes a press on the name is the one that closed it.
const REOPEN_GUARD: Duration = Duration::from_millis(250);
/// Blur radius the chevron comes out of, and the clear room around it the blur draws into.
const CHEVRON_BLUR: f64 = 3.;
const CHEVRON_PAD: f64 = 6.;

/// What the user did in the toolbar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolbarEvent {
    /// Picked canvas (0) or list (1).
    Mode(usize),
    /// Picked a color for the box's icon, one of `colors` given to [`WindowToolbar::attach`].
    Color(usize),
    /// Picked an SF Symbol for the box's icon, one of `symbols` given to
    /// [`WindowToolbar::attach`].
    Symbol(String),
    /// Picked box `index` of the list given to [`WindowToolbar::set_boxes`].
    OpenBox(usize),
    NewBox,
    /// Renamed box `index` of the list given to [`WindowToolbar::set_boxes`].
    RenameBox(usize, String),
}

/// A box as the switcher menu shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoxLabel {
    pub name: String,
    pub symbol: String,
    pub color: String,
    /// How many items the box holds, shown at the end of its row.
    pub items: usize,
}

struct DelegateIvars {
    items: Vec<(&'static str, Retained<NSToolbarItem>)>,
}

define_class!(
    // Hears the pointer enter and leave a tracking area.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryHover"]
    #[ivars = Box<dyn Fn(bool)>]
    pub(crate) struct Hover;

    unsafe impl NSObjectProtocol for Hover {}

    impl Hover {
        #[unsafe(method(mouseEntered:))]
        fn entered(&self, _event: &NSEvent) {
            (self.ivars())(true);
        }

        #[unsafe(method(mouseExited:))]
        fn exited(&self, _event: &NSEvent) {
            (self.ivars())(false);
        }
    }
);

impl Hover {
    pub(crate) fn new(mtm: MainThreadMarker, on_hover: impl Fn(bool) + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Box::new(on_hover) as Box<dyn Fn(bool)>);
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    // Hands the toolbar its items.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryWindowToolbarDelegate"]
    #[ivars = DelegateIvars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSToolbarDelegate for Delegate {
        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item(
            &self,
            _toolbar: &NSToolbar,
            id: &NSToolbarItemIdentifier,
            _flag: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            let name = id.to_string();
            self.ivars()
                .items
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, item)| item.clone())
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn default_ids(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            ids()
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed_ids(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            ids()
        }
    }
);

/// The box on the left; the switch and the account on the right, each on its own glass: a space between items keeps the toolbar from merging them into one platter.
fn ids() -> Retained<NSArray<NSToolbarItemIdentifier>> {
    let flexible: Retained<NSString> = unsafe { NSToolbarFlexibleSpaceItemIdentifier }.copy();
    let space: Retained<NSString> = unsafe { NSToolbarSpaceItemIdentifier }.copy();
    NSArray::from_retained_slice(&[
        NSString::from_str(BOX),
        flexible,
        NSString::from_str(MODES),
        space,
        NSString::from_str(AVATAR),
    ])
}

define_class!(
    // Hears a popover start to open and start to close.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryPopoverWatcher"]
    #[ivars = Box<dyn Fn(bool)>]
    struct PopoverWatcher;

    unsafe impl NSObjectProtocol for PopoverWatcher {}

    unsafe impl NSPopoverDelegate for PopoverWatcher {
        #[unsafe(method(popoverWillShow:))]
        fn will_show(&self, _notification: &NSNotification) {
            (self.ivars())(true);
        }

        #[unsafe(method(popoverWillClose:))]
        fn will_close(&self, _notification: &NSNotification) {
            (self.ivars())(false);
        }
    }
);

impl PopoverWatcher {
    /// `on_change` hears whether the popover is open, the moment that changes.
    fn new(mtm: MainThreadMarker, on_change: impl Fn(bool) + 'static) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Box::new(on_change) as Box<dyn Fn(bool)>);
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    // The chevron's image view. AppKit pins a view's layer on its corner again whenever it
    // lays the view out, so the turn is put back after every layout from the state it is
    // bound to, instead of being set once.
    #[unsafe(super(NSImageView))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryChevron"]
    #[ivars = Cell<bool>]
    struct ChevronView;

    impl ChevronView {
        #[unsafe(method(layout))]
        fn layout(&self) {
            unsafe {
                let _: () = msg_send![super(self), layout];
            }
            self.pin();
        }
    }
);

impl ChevronView {
    /// Whether the chevron points up, and whether that just changed.
    fn set_up(&self, up: bool) -> bool {
        self.ivars().replace(up) != up
    }

    /// Spins about the middle, pointing where the state says.
    fn pin(&self) {
        let Some(layer) = self.layer() else {
            return;
        };
        let frame = self.frame();
        let angle = if self.ivars().get() {
            std::f64::consts::PI
        } else {
            0.
        };
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        layer.setAnchorPoint(NSPoint::new(0.5, 0.5));
        layer.setPosition(NSPoint::new(
            frame.origin.x + frame.size.width / 2.,
            frame.origin.y + frame.size.height / 2.,
        ));
        layer.setTransform(unsafe { CATransform3DIdentity }.rotate(angle, 0., 0., 1.));
        CATransaction::commit();
    }
}

/// What the toolbar's controls share: the box list and the icon picker.
struct State {
    on_event: Rc<dyn Fn(ToolbarEvent)>,
    boxes: RefCell<Vec<BoxLabel>>,
    current: Cell<Option<usize>>,
    tile: Retained<NSView>,
    glyph: Retained<NSImageView>,
    name: Retained<NSTextField>,
    picker: Rc<IconPicker>,
    list: BoxList,
    chevron: Chevron,
    /// When the box list last started to close.
    list_closed: Cell<Option<Instant>>,
}

impl State {
    fn toggle_picker(&self) {
        let popover = &self.picker.popover;
        if popover.isShown() {
            unsafe { popover.performClose(None) };
        } else {
            popover.showRelativeToRect_ofView_preferredEdge(
                self.tile.bounds(),
                &self.tile,
                NSRectEdge::MaxY,
            );
        }
    }

    fn toggle_boxes(&self) {
        if self.list.popover.isShown() {
            self.list.close();
            return;
        }
        // The press that closes a transient popover reaches the name too: it closed the list
        // a moment ago, and must not open it again.
        if self
            .list_closed
            .get()
            .is_some_and(|closed| closed.elapsed() < REOPEN_GUARD)
        {
            return;
        }
        let mtm = MainThreadMarker::from(&*self.name);
        self.list.show(
            mtm,
            &self.name,
            &self.boxes.borrow(),
            self.current.get(),
            self.on_event.clone(),
        );
    }
}

/// A light that sweeps over `label` again and again, the way a thinking agent's status
/// shimmers: a mask brighter in a band that slides from left to right.
fn sweep(label: &NSTextField) {
    label.setWantsLayer(true);
    let (Some(layer), Some(gradient), Some(basic)) = (
        label.layer(),
        AnyClass::get(c"CAGradientLayer"),
        AnyClass::get(c"CABasicAnimation"),
    ) else {
        return;
    };
    let dim = NSColor::colorWithWhite_alpha(0., 0.45).CGColor();
    let bright = NSColor::colorWithWhite_alpha(0., 1.).CGColor();
    let colors = NSMutableArray::<AnyObject>::new();
    for color in [&dim, &bright, &dim] {
        colors.addObject(crate::tilt::bridged(&**color));
    }
    let numbers = |values: [f64; 3]| NSArray::from_retained_slice(&values.map(NSNumber::new_f64));
    unsafe {
        let mask: Retained<CALayer> = msg_send![gradient, layer];
        let _: () = msg_send![&*mask, setColors: &*colors];
        let _: () = msg_send![&*mask, setStartPoint: NSPoint::new(0., 0.5)];
        let _: () = msg_send![&*mask, setEndPoint: NSPoint::new(1., 0.5)];
        let _: () = msg_send![&*mask, setLocations: &*numbers([0., 0.15, 0.3])];
        mask.setFrame(label.bounds());
        layer.setMask(Some(&mask));
        let animation: Retained<AnyObject> =
            msg_send![basic, animationWithKeyPath: &*NSString::from_str("locations")];
        let _: () = msg_send![&*animation, setFromValue: &*numbers([-0.3, -0.15, 0.])];
        let _: () = msg_send![&*animation, setToValue: &*numbers([1., 1.15, 1.3])];
        let _: () = msg_send![&*animation, setDuration: SWEEP];
        let _: () = msg_send![&*animation, setRepeatCount: f32::INFINITY];
        let _: () =
            msg_send![&*mask, addAnimation: &*animation, forKey: &*NSString::from_str("sweep")];
    }
}

/// Sizes the sweep's mask to the label's text after it changes.
fn fit_sweep(label: &NSTextField) {
    let Some(mask) = label.layer().and_then(|layer| layer.mask()) else {
        return;
    };
    let size = label.fittingSize();
    if size.width <= 0. || size.height <= 0. {
        return;
    }
    // The band leans like a "/" by the lean from upright. A gradient's bands stand square to
    // its axis in the layer's unit square, which the text's shape then stretches, so the
    // axis drops by the lean's tangent scaled by the height over the width.
    let drop = SWEEP_LEAN.to_radians().tan() * size.height / size.width / 2.;
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    mask.setFrame(NSRect::new(NSPoint::ZERO, size));
    unsafe {
        let _: () = msg_send![&*mask, setStartPoint: NSPoint::new(0., 0.5 + drop)];
        let _: () = msg_send![&*mask, setEndPoint: NSPoint::new(1., 0.5 - drop)];
    }
    CATransaction::commit();
}

/// The chevron after the box's name: it comes out of a blur while the pointer is over the
/// name or the box list is open, pushing the status aside.
struct Chevron {
    image: Retained<ChevronView>,
    width: Retained<NSLayoutConstraint>,
    /// Width of the empty view after the status, which gives the chevron its room.
    spare: Retained<NSLayoutConstraint>,
    open_width: f64,
    inside: Cell<bool>,
    shown: Cell<bool>,
    /// Whether the box list is open, as its popover last reported: the chevron points up
    /// while it is.
    open: Cell<bool>,
}

impl Chevron {
    fn update(&self) {
        self.turn(self.open.get());
        let show = self.inside.get() || self.open.get();
        if self.shown.replace(show) == show {
            return;
        }
        NSAnimationContext::beginGrouping();
        let context = NSAnimationContext::currentContext();
        context.setDuration(CHEVRON_DURATION);
        context.setAllowsImplicitAnimation(true);
        // The stack's layout can't take a spring; this curve settles the way one does.
        context.setTimingFunction(Some(&CAMediaTimingFunction::functionWithControlPoints(
            0.2, 0.9, 0.25, 1.,
        )));
        // The chevron's room and the spare room after the status trade places, so the
        // toolbar item keeps its width and only the status moves.
        let open = if show { self.open_width } else { 0. };
        self.width.animator().setConstant(open);
        self.spare.animator().setConstant(self.open_width - open);
        NSAnimationContext::endGrouping();

        let (alpha, blur) = if show {
            ((0., 1.), (CHEVRON_BLUR, 0.))
        } else {
            ((1., 0.), (0., CHEVRON_BLUR))
        };
        self.image.setAlphaValue(alpha.1);
        let Some(layer) = self.image.layer() else {
            return;
        };
        spring(&layer, "opacity", alpha.0, alpha.1, CHEVRON_DURATION);
        if add_blur(&self.image) {
            spring(
                &layer,
                "filters.blur.inputRadius",
                blur.0,
                blur.1,
                CHEVRON_DURATION,
            );
        }
    }

    /// Turns the chevron up while the list is open and back down after.
    fn turn(&self, up: bool) {
        if !self.image.set_up(up) {
            return;
        }
        let Some(layer) = self.image.layer() else {
            return;
        };
        self.image.pin();
        let (from, to) = if up {
            (0., std::f64::consts::PI)
        } else {
            (std::f64::consts::PI, 0.)
        };
        spring(&layer, "transform.rotation.z", from, to, CHEVRON_DURATION);
    }
}

/// Gives `view` a Gaussian blur named "blur" for animating; false when Core Image is not
/// there.
fn add_blur(view: &NSView) -> bool {
    let Some(layer) = view.layer() else {
        return false;
    };
    if layer.filters().is_some_and(|filters| filters.count() > 0) {
        return true;
    }
    let Some(class) = AnyClass::get(c"CIFilter") else {
        return false;
    };
    let filter: Option<Retained<AnyObject>> =
        unsafe { msg_send![class, filterWithName: &*NSString::from_str("CIGaussianBlur")] };
    let Some(filter) = filter else {
        return false;
    };
    unsafe {
        let _: () = msg_send![&*filter, setDefaults];
        let _: () = msg_send![&*filter, setName: &*NSString::from_str("blur")];
    }
    view.setLayerUsesCoreImageFilters(true);
    let filters = NSArray::from_retained_slice(&[filter]);
    unsafe {
        let _: () = msg_send![view, setContentFilters: &*filters];
    }
    true
}

/// The toolbar of the window that holds `ns_view`.
pub struct WindowToolbar {
    state: Rc<State>,
    modes: Retained<NSTabView>,
    status: Retained<NSTextField>,
    account: Retained<NSButton>,
    _delegate: Retained<Delegate>,
    _targets: Vec<Retained<Target>>,
    /// The tracking area's owner and the box list's delegate, which AppKit does not retain.
    _hover: Retained<Hover>,
    _list_watcher: Retained<PopoverWatcher>,
}

impl WindowToolbar {
    /// `colors` and `symbols` are what the icon picker offers, the symbols in named groups;
    /// a picked color comes back as its index.
    ///
    /// # Safety
    /// `ns_view` must point to a live `NSView` in a window, and this must run on the main
    /// thread.
    pub unsafe fn attach(
        ns_view: NonNull<c_void>,
        colors: &[&str],
        symbols: &[(&str, &[&str])],
        avatar: Option<&Path>,
        initial: &str,
        on_event: impl Fn(ToolbarEvent) + 'static,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let view: &NSView = unsafe { ns_view.cast().as_ref() };
        let window = view.window()?;

        let on_event: Rc<dyn Fn(ToolbarEvent)> = Rc::new(on_event);

        // The box: a colored tile with its glyph, the name, a chevron that shows under the
        // pointer, and a grey status.
        let tile = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(TILE, TILE)),
        );
        tile.setWantsLayer(true);
        if let Some(layer) = tile.layer() {
            layer.setCornerRadius(6.);
        }
        tile.setTranslatesAutoresizingMaskIntoConstraints(false);
        tile.widthAnchor()
            .constraintEqualToConstant(TILE)
            .setActive(true);
        tile.heightAnchor()
            .constraintEqualToConstant(TILE)
            .setActive(true);
        tile.setToolTip(Some(&NSString::from_str("Change icon")));
        let glyph = NSImageView::initWithFrame(NSImageView::alloc(mtm), NSRect::ZERO);
        glyph.setImageScaling(NSImageScaling::ScaleNone);
        glyph.setContentTintColor(Some(&NSColor::whiteColor()));
        tile.addSubview(&glyph);

        let title_font = NSFont::systemFontOfSize(TITLE_SIZE);
        // A label, not a button: a button pads its title by several points on each side.
        let name = NSTextField::labelWithString(&NSString::new(), mtm);
        name.setFont(Some(&title_font));
        name.setTextColor(Some(&NSColor::labelColor()));
        name.setToolTip(Some(&NSString::from_str("Switch box")));

        // The chevron is smaller than the name and a step heavier, so its strokes come out as
        // thick as the text's.
        let chevron_image = symbol("chevron.down", "Boxes").and_then(|image| {
            let config = NSImageSymbolConfiguration::configurationWithPointSize_weight_scale(
                CHEVRON_SIZE,
                unsafe { objc2_app_kit::NSFontWeightMedium },
                NSImageSymbolScale::Small,
            );
            image.imageWithSymbolConfiguration(&config)
        });
        let chevron_size = chevron_image
            .as_ref()
            .map_or(NSSize::new(10., 10.), |image| image.size());
        // The chevron sits on a clear square with room for its blur, turning about its middle.
        let side = chevron_size.width.max(chevron_size.height) + CHEVRON_PAD * 2.;
        let chevron: Retained<ChevronView> = unsafe {
            let this = ChevronView::alloc(mtm).set_ivars(Cell::new(false));
            msg_send![super(this), initWithFrame: NSRect::ZERO]
        };
        chevron.setImage(
            chevron_image
                .as_deref()
                .map(|image| centered_symbol(image, side))
                .as_deref(),
        );
        chevron.setImageScaling(NSImageScaling::ScaleNone);
        chevron.setContentTintColor(Some(&NSColor::labelColor()));
        chevron.setWantsLayer(true);
        chevron.setAlphaValue(0.);
        chevron.setTranslatesAutoresizingMaskIntoConstraints(false);
        // The chevron sits in a holder that opens from nothing, so the status slides aside.
        let holder = NSView::initWithFrame(NSView::alloc(mtm), NSRect::ZERO);
        holder.setTranslatesAutoresizingMaskIntoConstraints(false);
        holder.addSubview(&chevron);
        chevron
            .centerXAnchor()
            .constraintEqualToAnchor_constant(
                &holder.leadingAnchor(),
                CHEVRON_GAP + chevron_size.width / 2.,
            )
            .setActive(true);
        chevron
            .centerYAnchor()
            .constraintEqualToAnchor(&holder.centerYAnchor())
            .setActive(true);
        chevron
            .widthAnchor()
            .constraintEqualToConstant(side)
            .setActive(true);
        chevron
            .heightAnchor()
            .constraintEqualToConstant(side)
            .setActive(true);
        holder
            .heightAnchor()
            .constraintEqualToConstant(chevron_size.height)
            .setActive(true);
        let open_width = CHEVRON_GAP + chevron_size.width;
        let holder_width = holder.widthAnchor().constraintEqualToConstant(0.);
        holder_width.setActive(true);
        if let Some(layer) = {
            holder.setWantsLayer(true);
            holder.layer()
        } {
            layer.setMasksToBounds(false);
        }

        let status = NSTextField::labelWithString(&NSString::new(), mtm);
        status.setFont(Some(&NSFont::systemFontOfSize(TITLE_SIZE)));
        status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        sweep(&status);
        // An empty view after the status gives the chevron its room while it is hidden and
        // shrinks as the chevron opens, so the item keeps its width and both move together.
        let spare_view = NSView::initWithFrame(NSView::alloc(mtm), NSRect::ZERO);
        spare_view.setTranslatesAutoresizingMaskIntoConstraints(false);
        let spare = spare_view
            .widthAnchor()
            .constraintEqualToConstant(open_width);
        spare.setActive(true);
        let stack = NSStackView::stackViewWithViews(
            &NSArray::from_retained_slice(&[
                tile.clone(),
                Retained::into_super(Retained::into_super(name.clone())),
                holder.clone(),
                Retained::into_super(Retained::into_super(status.clone())),
                spare_view,
            ]),
            mtm,
        );
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        stack.setSpacing(8.);
        stack.setCustomSpacing_afterView(0., &name);
        stack.setCustomSpacing_afterView(0., &status);
        // Same size, so the name and the status share one baseline.
        let baseline = status
            .firstBaselineAnchor()
            .constraintEqualToAnchor(&name.firstBaselineAnchor());
        baseline.setPriority(999.);
        baseline.setActive(true);
        // Shortcuts keeps its icon 16 points from the traffic lights.
        stack.setEdgeInsets(objc2_foundation::NSEdgeInsets {
            top: 0.,
            left: 5.,
            bottom: 0.,
            right: 0.,
        });

        let state = Rc::new(State {
            on_event: on_event.clone(),
            boxes: RefCell::new(Vec::new()),
            current: Cell::new(None),
            tile: tile.clone(),
            glyph,
            name: name.clone(),
            picker: IconPicker::new(mtm, colors, symbols, on_event.clone()),
            list: BoxList::new(mtm),
            chevron: Chevron {
                image: chevron,
                width: holder_width,
                spare,
                open_width,
                inside: Cell::new(false),
                shown: Cell::new(false),
                open: Cell::new(false),
            },
            list_closed: Cell::new(None),
        });

        let hover = Hover::new(mtm, {
            let state = Rc::downgrade(&state);
            move |inside| {
                if let Some(state) = state.upgrade() {
                    state.chevron.inside.set(inside);
                    state.chevron.update();
                }
            }
        });
        let tracking = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::ActiveAlways
                    | NSTrackingAreaOptions::InVisibleRect,
                Some(&hover),
                None,
            )
        };
        name.addTrackingArea(&tracking);
        let list_watcher = PopoverWatcher::new(mtm, {
            let state = Rc::downgrade(&state);
            move |open| {
                if let Some(state) = state.upgrade() {
                    if !open {
                        state.list_closed.set(Some(Instant::now()));
                    }
                    state.chevron.open.set(open);
                    state.chevron.update();
                }
            }
        });
        state
            .list
            .popover
            .setDelegate(Some(ProtocolObject::from_ref(&*list_watcher)));

        let tile_target = Target::new(mtm, {
            let state = state.clone();
            move |_| state.toggle_picker()
        });
        let click = unsafe {
            NSClickGestureRecognizer::initWithTarget_action(
                NSClickGestureRecognizer::alloc(mtm),
                Some(&tile_target),
                Some(sel!(fire:)),
            )
        };
        tile.addGestureRecognizer(&click);
        let name_target = Target::new(mtm, {
            let state = state.clone();
            move |_| state.toggle_boxes()
        });
        let name_click = unsafe {
            NSClickGestureRecognizer::initWithTarget_action(
                NSClickGestureRecognizer::alloc(mtm),
                Some(&name_target),
                Some(sel!(fire:)),
            )
        };
        name.addGestureRecognizer(&name_click);

        let box_item = item(mtm, BOX, "Box");
        box_item.setView(Some(&stack));
        box_item.setBordered(false);

        // Canvas and list: the tab strip of a real NSTabView, which is the control that lifts
        // the glass bubble while pressed. The strip alone goes in the toolbar; the tab view
        // stays out of the window and keeps driving it.
        let images = icon_images(&[
            ("squareshape.split.3x3", "Canvas"),
            ("rectangle.split.3x1", "Columns"),
        ])?;
        let tabs = icon_tab_view(mtm, images.len(), &images, 0.);
        let strip = segments(&tabs)?;
        strip.removeFromSuperview();
        tabs.setControlSize(NSControlSize::ExtraLarge);
        size_modes(&strip, &images);
        let modes_target = Target::with_tabs(
            mtm,
            {
                let state = state.clone();
                let strip = strip.clone();
                move |sender| {
                    let Some(tabs) = sender.downcast_ref::<NSTabView>() else {
                        return;
                    };
                    // NSTabView may rebuild its segments on selection; put the icons back.
                    size_modes(&strip, &images);
                    if let Some(index) = selected_index(tabs) {
                        (state.on_event)(ToolbarEvent::Mode(index));
                    }
                }
            },
            |_, _| true,
        );
        tabs.setDelegate(Some(ProtocolObject::from_ref(&*modes_target)));
        let modes_item = item(mtm, MODES, "View");
        modes_item.setView(Some(&strip));

        let account = account_button(mtm);
        show_account(&account, avatar, initial);
        let menu_targets = account_targets(mtm);
        let account_target = Target::new(mtm, {
            let targets = menu_targets.clone();
            move |sender| {
                if let Some(button) = sender.downcast_ref::<NSButton>() {
                    // Under the button, a little way down, with the menu's right edge on
                    // the button's right edge.
                    let menu = account_menu(mtm, &targets);
                    let bounds = button.bounds();
                    menu.popUpMenuPositioningItem_atLocation_inView(
                        None,
                        NSPoint::new(
                            bounds.size.width - menu.size().width,
                            if button.isFlipped() {
                                bounds.size.height + MENU_GAP
                            } else {
                                -MENU_GAP
                            },
                        ),
                        Some(button),
                    );
                }
            }
        });
        unsafe {
            account.setTarget(Some(&account_target));
            account.setAction(Some(sel!(fire:)));
        }
        let avatar_item = item(mtm, AVATAR, "Account");
        avatar_item.setView(Some(&account));

        let delegate: Retained<Delegate> = {
            let this = Delegate::alloc(mtm).set_ivars(DelegateIvars {
                items: vec![(BOX, box_item), (MODES, modes_item), (AVATAR, avatar_item)],
            });
            unsafe { msg_send![super(this), init] }
        };
        let toolbar = NSToolbar::initWithIdentifier(
            NSToolbar::alloc(mtm),
            &NSString::from_str("ambry.window"),
        );
        toolbar.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        toolbar.setAllowsUserCustomization(false);
        toolbar.setDisplayMode(objc2_app_kit::NSToolbarDisplayMode::IconOnly);
        window.setToolbarStyle(NSWindowToolbarStyle::Unified);
        window.setToolbar(Some(&toolbar));
        Some(Self {
            state,
            modes: tabs,
            status,
            _delegate: delegate,
            _hover: hover,
            _list_watcher: list_watcher,
            account,
            _targets: [
                vec![tile_target, name_target, modes_target, account_target],
                menu_targets,
            ]
            .concat(),
        })
    }

    /// Selects canvas (0) or list (1) without reporting it.
    pub fn select_mode(&self, index: usize) {
        if selected_index(&self.modes) != Some(index) {
            self.modes.selectTabViewItemAtIndex(index as isize);
        }
    }

    /// Shows the open box: its icon, name and a status such as "3 items".
    pub fn set_box(&self, label: &BoxLabel, status: &str) {
        let state = &self.state;
        place_symbol(
            &state.glyph,
            glyph_image(&label.symbol, 12.).as_deref(),
            TILE,
        );
        paint_tile(&state.tile, &label.color, TILE, 6.);
        state.picker.mark(&label.color, &label.symbol);
        self.state
            .name
            .setStringValue(&NSString::from_str(&label.name));
        self.status.setStringValue(&NSString::from_str(status));
        fit_sweep(&self.status);
    }

    /// Shows the account's `picture`, or its `initial` when there is none.
    pub fn set_account(&self, picture: Option<&Path>, initial: &str) {
        show_account(&self.account, picture, initial);
    }

    /// The boxes the name's menu offers, and which of them is open.
    pub fn set_boxes(&self, boxes: Vec<BoxLabel>, current: Option<usize>) {
        *self.state.boxes.borrow_mut() = boxes;
        self.state.current.set(current);
    }
}

/// The switch at the size of the toolbar's other glass, like Freeform's Grid/List: extra
/// large, with segments of one width so the grey pill sits the same on either.
fn size_modes(strip: &NSSegmentedControl, images: &[Retained<NSImage>]) {
    decorate_strip(strip, images, MODE_WIDTH);
    strip.setControlSize(NSControlSize::ExtraLarge);
    strip.sizeToFit();
}

fn item(mtm: MainThreadMarker, id: &str, label: &str) -> Retained<NSToolbarItem> {
    let item =
        NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), &NSString::from_str(id));
    item.setLabel(&NSString::from_str(label));
    item
}

/// The color a box color name stands for: Shortcuts' icon colors.
pub(crate) fn box_color(name: &str) -> Retained<NSColor> {
    let rgb: u32 = match name {
        "red" => 0xe94f4a,
        "coral" => 0xee7e62,
        "orange" => 0xf0a04c,
        "yellow" => 0xe6c03a,
        "green" => 0x5bb85c,
        "teal" | "mint" => 0x2fbfa0,
        "sky" => 0x77c2e8,
        "indigo" => 0x4b5fc4,
        "violet" | "purple" => 0x7a4cc4,
        "lavender" | "magenta" => 0xa66bd9,
        "pink" => 0xe27fc6,
        "gray" => 0x78808a,
        "sage" => 0x8da58f,
        "taupe" => 0xb19b80,
        _ => 0x3d83f0,
    };
    let channel = |shift: u32| f64::from((rgb >> shift) & 0xff) / 255.;
    NSColor::colorWithSRGBRed_green_blue_alpha(channel(16), channel(8), channel(0), 1.)
}

/// A semibold SF Symbol at `size` points.
pub(crate) fn sized_symbol(name: &str, description: &str, size: f64) -> Option<Retained<NSImage>> {
    let image = symbol(name, description)?;
    let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(size, unsafe {
        objc2_app_kit::NSFontWeightSemibold
    });
    image.imageWithSymbolConfiguration(&config)
}

/// A box glyph at `size` points: the filled variant of the symbol when there is one, as
/// Shortcuts shows its glyphs.
pub(crate) fn glyph_image(name: &str, size: f64) -> Option<Retained<NSImage>> {
    let image = symbol(&format!("{name}.fill"), name).or_else(|| symbol(name, name))?;
    let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(size, unsafe {
        objc2_app_kit::NSFontWeightMedium
    });
    image.imageWithSymbolConfiguration(&config)
}

/// Puts `image`, a symbol, in `view` as a `size` square with the drawn glyph in its middle.
/// SF Symbols carry room for descenders and baselines that leave the glyph off center when
/// the image itself is centered, so the glyph is centered by its inked pixels instead.
pub(crate) fn place_symbol(view: &NSImageView, image: Option<&NSImage>, size: f64) {
    view.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(size, size)));
    view.setImage(image.map(|image| centered_symbol(image, size)).as_deref());
}

/// `image` drawn on a clear `side` square so its ink sits in the middle, as a template image
/// that takes its view's tint.
///
/// The image carries one bitmap for each screen density. On each, the glyph's ink lands on
/// whole pixels: a glyph 15 pixels wide can't sit in the middle of a 26 pixel tile, so it is
/// drawn a touch larger or smaller until its width and height leave even margins.
pub(crate) fn centered_symbol(image: &NSImage, side: f64) -> Retained<NSImage> {
    let size = image.size();
    let ink = ink_bounds(image).unwrap_or(NSRect::new(NSPoint::ZERO, size));
    let centered = NSImage::initWithSize(NSImage::alloc(), NSSize::new(side, side));
    for scale in [1., 2.] {
        let pixels = (side * scale).round();
        let fit = fit_ink(ink.size.width * scale, ink.size.height * scale, pixels);
        let (width, height) = (ink.size.width * scale * fit, ink.size.height * scale * fit);
        let left = ((pixels - width) / 2.).round();
        let bottom = ((pixels - height) / 2.).round();
        let rect = NSRect::new(
            NSPoint::new(
                left - ink.origin.x * scale * fit,
                bottom - ink.origin.y * scale * fit,
            ),
            NSSize::new(size.width * scale * fit, size.height * scale * fit),
        );
        if let Some(rep) = draw_bitmap(image, pixels as isize, pixels as isize, rect) {
            rep.setSize(NSSize::new(side, side));
            centered.addRepresentation(&rep);
        }
    }
    centered.setTemplate(true);
    centered
}

/// How much to grow or shrink ink `width` by `height` pixels so both leave even margins in a
/// square of `pixels`, changing its size as little as it can.
fn fit_ink(width: f64, height: f64, pixels: f64) -> f64 {
    // How far `length` is from a whole number of pixels that leaves even margins.
    let miss = |length: f64| {
        let even = |n: f64| (pixels - n) % 2. == 0.;
        let below = length.floor();
        [below - 1., below, below + 1., below + 2.]
            .into_iter()
            .filter(|n| even(*n))
            .map(|n| (n - length).abs())
            .fold(f64::MAX, f64::min)
    };
    let mut candidates = vec![1.];
    for length in [width, height] {
        let below = length.floor();
        for n in [below - 1., below, below + 1., below + 2.] {
            if n > 0. && (pixels - n) % 2. == 0. {
                candidates.push(n / length);
            }
        }
    }
    candidates
        .into_iter()
        .filter(|fit| (0.9..=1.1).contains(fit))
        .map(|fit| {
            let cost = miss(width * fit) + miss(height * fit) + 4. * (fit - 1.).abs() * width;
            (fit, cost)
        })
        .fold(
            (1., f64::MAX),
            |best, next| if next.1 < best.1 { next } else { best },
        )
        .0
}

/// `image` drawn into `rect` of a clear bitmap `width` by `height` pixels.
fn draw_bitmap(
    image: &NSImage,
    width: isize,
    height: isize,
    rect: NSRect,
) -> Option<Retained<NSBitmapImageRep>> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            width,
            height,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    image.drawInRect_fromRect_operation_fraction(
        rect,
        NSRect::ZERO,
        NSCompositingOperation::SourceOver,
        1.,
    );
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    Some(rep)
}

/// The part of `image` that is drawn on, in its points from the bottom left. The image is
/// drawn eight times over and a pixel counts when it is at least half covered, which puts
/// the edges where the eye sees them.
fn ink_bounds(image: &NSImage) -> Option<NSRect> {
    const SCALE: f64 = 8.;
    let size = image.size();
    let width = (size.width * SCALE).ceil() as isize;
    let height = (size.height * SCALE).ceil() as isize;
    let rect = NSRect::new(
        NSPoint::ZERO,
        NSSize::new(size.width * SCALE, size.height * SCALE),
    );
    let rep = draw_bitmap(image, width, height, rect)?;
    let data = rep.bitmapData();
    if data.is_null() {
        return None;
    }
    let row_bytes = rep.bytesPerRow();
    let (mut left, mut right, mut top, mut bottom) = (isize::MAX, -1, isize::MAX, -1);
    for y in 0..height {
        for x in 0..width {
            let alpha = unsafe { *data.offset(y * row_bytes + x * 4 + 3) };
            if alpha >= 128 {
                left = left.min(x);
                right = right.max(x);
                top = top.min(y);
                bottom = bottom.max(y);
            }
        }
    }
    if right < 0 {
        return None;
    }
    // Bitmap rows run from the top; the image counts from the bottom.
    Some(NSRect::new(
        NSPoint::new(left as f64 / SCALE, (height - 1 - bottom) as f64 / SCALE),
        NSSize::new(
            (right - left + 1) as f64 / SCALE,
            (bottom - top + 1) as f64 / SCALE,
        ),
    ))
}

/// Fills a box's tile with its color as a soft top-to-bottom gradient, like a shortcut's
/// icon in Shortcuts.
pub(crate) fn paint_tile(tile: &NSView, color: &str, size: f64, radius: f64) {
    let Some(layer) = tile.layer() else {
        return;
    };
    layer.setCornerRadius(radius);
    let base = box_color(color);
    let top = base
        .blendedColorWithFraction_ofColor(0.2, &NSColor::whiteColor())
        .unwrap_or_else(|| base.clone());
    let bottom = base
        .blendedColorWithFraction_ofColor(0.08, &NSColor::blackColor())
        .unwrap_or_else(|| base.clone());
    let Some(class) = AnyClass::get(c"CAGradientLayer") else {
        layer.setBackgroundColor(Some(&base.CGColor()));
        return;
    };
    let name = NSString::from_str("ambry.fill");
    let existing = unsafe { layer.sublayers() }.and_then(|sublayers| {
        sublayers
            .iter()
            .find(|sublayer| sublayer.name().is_some_and(|n| *n == *name))
    });
    let gradient: Retained<AnyObject> = match existing {
        Some(sublayer) => Retained::into_super(Retained::into_super(sublayer)),
        None => unsafe {
            let gradient: Retained<AnyObject> = msg_send![class, layer];
            let _: () = msg_send![&*gradient, setName: &*name];
            let _: () = msg_send![&*layer, insertSublayer: &*gradient, atIndex: 0u32];
            gradient
        },
    };
    let (top, bottom) = (top.CGColor(), bottom.CGColor());
    let colors = NSMutableArray::<AnyObject>::new();
    for color in [&bottom, &top] {
        // CGColor is toll-free bridged to an Objective-C object.
        let object: &AnyObject = unsafe { &*((&**color) as *const _ as *const AnyObject) };
        colors.addObject(object);
    }
    unsafe {
        let _: () = msg_send![
            &*gradient,
            setFrame: NSRect::new(NSPoint::ZERO, NSSize::new(size, size))
        ];
        let _: () = msg_send![&*gradient, setCornerRadius: radius];
        let _: () = msg_send![&*gradient, setColors: &*colors];
    }
    layer.setBackgroundColor(None);
}

/// Runs a spring on `layer`'s `key_path` from `from` to `to`, as SwiftUI's default spring
/// over `duration` seconds, and leaves the layer at `to`.
pub(crate) fn spring(layer: &AnyObject, key_path: &str, from: f64, to: f64, duration: f64) {
    bouncy_spring(layer, key_path, from, to, duration, 0.);
}

/// [`spring`] with `bounce`: 0 settles without overshoot, 0.3 overshoots a little.
pub(crate) fn bouncy_spring(
    layer: &AnyObject,
    key_path: &str,
    from: f64,
    to: f64,
    duration: f64,
    bounce: f64,
) {
    let key = NSString::from_str(key_path);
    let to_value = NSNumber::new_f64(to);
    unsafe {
        let _: () = msg_send![layer, setValue: &*to_value, forKeyPath: &*key];
        let Some(class) = AnyClass::get(c"CASpringAnimation") else {
            return;
        };
        let animation: Allocated<AnyObject> = msg_send![class, alloc];
        let animation: Retained<AnyObject> =
            msg_send![animation, initWithPerceptualDuration: duration, bounce: bounce];
        let _: () = msg_send![&*animation, setKeyPath: &*key];
        let _: () = msg_send![&*animation, setFromValue: &*NSNumber::new_f64(from)];
        let _: () = msg_send![&*animation, setToValue: &*to_value];
        let settle: f64 = msg_send![&*animation, settlingDuration];
        let _: () = msg_send![&*animation, setDuration: settle];
        let _: () = msg_send![layer, addAnimation: &*animation, forKey: &*key];
    }
}

/// The account picture cut to a circle, or the user's initial as a button, for the toolbar
/// to put on glass.
fn account_button(mtm: MainThreadMarker) -> Retained<NSButton> {
    // A plain bordered button, which the toolbar puts on glass like its other buttons.
    let button =
        unsafe { NSButton::buttonWithTitle_target_action(&NSString::new(), None, None, mtm) };
    button.setImageScaling(objc2_app_kit::NSImageScaling::ScaleProportionallyUpOrDown);
    button.setToolTip(Some(&NSString::from_str("Account")));
    // A circle, as wide as it is tall: a bordered button pads its sides otherwise.
    button.setTranslatesAutoresizingMaskIntoConstraints(false);
    button
        .widthAnchor()
        .constraintEqualToAnchor(&button.heightAnchor())
        .setActive(true);
    button
}

fn show_account(button: &NSButton, picture: Option<&Path>, initial: &str) {
    let image = picture
        .filter(|path| path.exists())
        .and_then(|path| {
            NSImage::initWithContentsOfFile(
                NSImage::alloc(),
                &NSString::from_str(&path.to_string_lossy()),
            )
        })
        .map(|image| round_image(&image, AVATAR_SIZE));
    match image {
        Some(image) => {
            button.setTitle(&NSString::new());
            button.setImage(Some(&image));
            button.setImagePosition(objc2_app_kit::NSCellImagePosition::ImageOnly);
        }
        None => {
            button.setImage(None);
            button.setImagePosition(objc2_app_kit::NSCellImagePosition::NoImage);
            button.setTitle(&NSString::from_str(initial));
        }
    }
}

/// `image` filling a circle `side` points across, with a bitmap for each screen density.
fn round_image(image: &NSImage, side: f64) -> Retained<NSImage> {
    let size = image.size();
    let round = NSImage::initWithSize(NSImage::alloc(), NSSize::new(side, side));
    for scale in [1., 2.] {
        let pixels = side * scale;
        let fill = (pixels / size.width).max(pixels / size.height);
        let drawn = NSSize::new(size.width * fill, size.height * fill);
        let rect = NSRect::new(
            NSPoint::new((pixels - drawn.width) / 2., (pixels - drawn.height) / 2.),
            drawn,
        );
        let clip = NSRect::new(NSPoint::ZERO, NSSize::new(pixels, pixels));
        if let Some(rep) = draw_clipped(image, pixels as isize, rect, clip) {
            rep.setSize(NSSize::new(side, side));
            round.addRepresentation(&rep);
        }
    }
    round
}

/// `image` drawn into `rect` of a clear square bitmap `pixels` across, inside the oval in
/// `clip`.
fn draw_clipped(
    image: &NSImage,
    pixels: isize,
    rect: NSRect,
    clip: NSRect,
) -> Option<Retained<NSBitmapImageRep>> {
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            pixels,
            pixels,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            0,
            0,
        )
    }?;
    let context = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    objc2_app_kit::NSBezierPath::bezierPathWithOvalInRect(clip).addClip();
    image.drawInRect_fromRect_operation_fraction(
        rect,
        NSRect::ZERO,
        NSCompositingOperation::SourceOver,
        1.,
    );
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    Some(rep)
}

/// What the account menu's items do, in the order [`account_menu`] uses them.
fn account_targets(mtm: MainThreadMarker) -> Vec<Retained<Target>> {
    let pane = |pane: Pane| Target::new(mtm, move |_| settings::open(pane));
    let link = |url: &'static str| {
        Target::new(mtm, move |_| {
            if let Some(url) = NSURL::URLWithString(&NSString::from_str(url)) {
                NSWorkspace::sharedWorkspace().openURL(&url);
            }
        })
    };
    vec![
        pane(Pane::Account),
        pane(Pane::General),
        pane(Pane::Ai),
        pane(Pane::Connections),
        link("https://github.com/victorpapou/ambry"),
        link("https://github.com/victorpapou/ambry/issues/new"),
    ]
}

/// The menu under the account picture: who is signed in, then Settings and its panes, then
/// help.
fn account_menu(mtm: MainThreadMarker, targets: &[Retained<Target>]) -> Retained<NSMenu> {
    let menu = NSMenu::new(mtm);
    let header = NSMenuItem::new(mtm);
    header.setTitle(&NSString::from_str(&settings::user_name()));
    header.setEnabled(false);
    menu.addItem(&header);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let entries: [(&str, &str, &str); 6] = [
        ("Account…", "person.crop.circle", ""),
        ("Settings…", "gearshape", ","),
        ("AI Model…", "sparkles", ""),
        ("Connections…", "puzzlepiece.extension", ""),
        ("Ambry on GitHub", "questionmark.circle", ""),
        ("Report a Problem…", "exclamationmark.bubble", ""),
    ];
    for (index, ((title, glyph, key), target)) in entries.iter().zip(targets).enumerate() {
        if index == 4 {
            menu.addItem(&NSMenuItem::separatorItem(mtm));
        }
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &NSString::from_str(title),
                Some(sel!(fire:)),
                &NSString::from_str(key),
            )
        };
        unsafe { item.setTarget(Some(target)) };
        item.setImage(symbol(glyph, title).as_deref());
        menu.addItem(&item);
    }
    menu
}
