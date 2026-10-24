//! The window's own NSToolbar: the box's icon and name on the left, and the add button, the
//! canvas/list switch and the account on the right. macOS draws them with the toolbar's glass,
//! grey selection and press bubble.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::path::Path;
use std::ptr::NonNull;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSBox, NSBoxType, NSButton, NSClickGestureRecognizer, NSColor, NSControlStateValueOn, NSFont,
    NSImage, NSImageScaling, NSImageSymbolConfiguration, NSImageView, NSPopover, NSPopoverBehavior,
    NSSegmentSwitchTracking, NSSegmentedControl, NSStackView, NSTabView, NSTextField, NSToolbar,
    NSToolbarDelegate, NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem,
    NSToolbarItemIdentifier, NSToolbarSpaceItemIdentifier, NSUserInterfaceLayoutOrientation,
    NSView, NSViewController, NSWindowToolbarStyle,
};
use objc2_foundation::{NSArray, NSCopying, NSPoint, NSRect, NSRectEdge, NSSize, NSString};

use crate::native::{
    Target, decorate_strip, icon_images, icon_tab_view, segments, selected_index, symbol,
};

const BOX: &str = "box";
const ADD: &str = "add";
const MODES: &str = "modes";
const AVATAR: &str = "avatar";
/// Point size of the account picture inside its glass circle.
const AVATAR_SIZE: f64 = 28.;
/// The box's icon tile next to its name.
const TILE: f64 = 22.;
/// Swatches and glyphs in the icon picker, which is laid out like the one in Shortcuts.
const SWATCH: f64 = 32.;
const SWATCH_GAP: f64 = 12.;
const SWATCH_COLUMNS: usize = 5;
const GLYPH: f64 = 36.;
const GLYPH_COLUMNS: usize = 6;
const PICKER_PADDING: f64 = 16.;
/// Tags of the glyph buttons in the picker start here; colors use 0 up.
const GLYPH_TAG: isize = 100;
/// Tag of the "New Box" row in the box list.
const NEW_BOX_TAG: isize = -1;
/// Width of the box list's rows.
const ROW_WIDTH: f64 = 220.;
const ROW_HEIGHT: f64 = 28.;

/// What the user did in the toolbar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolbarEvent {
    /// Picked canvas (0) or list (1).
    Mode(usize),
    Add,
    /// Picked a color for the box's icon, one of `colors` given to [`WindowToolbar::attach`].
    Color(usize),
    /// Picked a glyph for the box's icon, one of `symbols` given to [`WindowToolbar::attach`].
    Symbol(usize),
    /// Picked box `index` of the list given to [`WindowToolbar::set_boxes`].
    OpenBox(usize),
    NewBox,
}

/// A box as the switcher menu shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoxLabel {
    pub name: String,
    pub symbol: String,
    pub color: String,
}

struct DelegateIvars {
    items: Vec<(&'static str, Retained<NSToolbarItem>)>,
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

/// The box on the left; the add button, the switch and the account on the right, each on its
/// own glass: a space between items keeps the toolbar from merging them into one platter.
fn ids() -> Retained<NSArray<NSToolbarItemIdentifier>> {
    let flexible: Retained<NSString> = unsafe { NSToolbarFlexibleSpaceItemIdentifier }.copy();
    let space: Retained<NSString> = unsafe { NSToolbarSpaceItemIdentifier }.copy();
    NSArray::from_retained_slice(&[
        NSString::from_str(BOX),
        flexible,
        NSString::from_str(ADD),
        space.clone(),
        NSString::from_str(MODES),
        space,
        NSString::from_str(AVATAR),
    ])
}

/// What the toolbar's controls share: the box list and the icon picker.
struct State {
    on_event: Box<dyn Fn(ToolbarEvent)>,
    boxes: RefCell<Vec<BoxLabel>>,
    current: Cell<Option<usize>>,
    tile: Retained<NSView>,
    glyph: Retained<NSImageView>,
    name: Retained<NSButton>,
    picker: Retained<NSPopover>,
    picker_view: Picker,
    list: Retained<NSPopover>,
}

impl State {
    fn toggle(popover: &NSPopover, anchor: &NSView) {
        if popover.isShown() {
            unsafe { popover.performClose(None) };
        } else {
            popover.showRelativeToRect_ofView_preferredEdge(
                anchor.bounds(),
                anchor,
                NSRectEdge::MaxY,
            );
        }
    }

    /// The list of boxes under the name, with the open box marked and "New Box" at the end,
    /// in a popover like Finder's share list.
    fn show_boxes(&self, target: &Target) {
        if self.list.isShown() {
            unsafe { self.list.performClose(None) };
            return;
        }
        let mtm = MainThreadMarker::from(target);
        let mut rows: Vec<Retained<NSView>> = Vec::new();
        for (index, label) in self.boxes.borrow().iter().enumerate() {
            let row = list_row(mtm, &label.name, target, index as isize);
            row.setImage(colored_symbol(&label.symbol, &label.color).as_deref());
            if self.current.get() == Some(index) {
                row.setState(NSControlStateValueOn);
                if let Some(check) = symbol("checkmark", "Open") {
                    row.setAlternateImage(Some(&check));
                }
                row.setFont(Some(&NSFont::systemFontOfSize_weight(13., unsafe {
                    objc2_app_kit::NSFontWeightSemibold
                })));
            }
            rows.push(Retained::into_super(Retained::into_super(row)));
        }
        let separator = NSBox::initWithFrame(
            NSBox::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(ROW_WIDTH, 1.)),
        );
        separator.setBoxType(NSBoxType::Separator);
        rows.push(Retained::into_super(separator));
        let new = list_row(mtm, "New Box", target, NEW_BOX_TAG);
        new.setImage(symbol("plus", "New Box").as_deref());
        rows.push(Retained::into_super(Retained::into_super(new)));

        let stack = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(&rows), mtm);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(objc2_app_kit::NSLayoutAttribute::Leading);
        stack.setSpacing(2.);
        stack.setEdgeInsets(objc2_foundation::NSEdgeInsets {
            top: 6.,
            left: 6.,
            bottom: 6.,
            right: 6.,
        });
        let size = stack.fittingSize();
        stack.setFrameSize(size);
        let controller =
            NSViewController::initWithNibName_bundle(NSViewController::alloc(mtm), None, None);
        controller.setView(&stack);
        self.list.setContentViewController(Some(&controller));
        self.list.setContentSize(size);
        self.list.showRelativeToRect_ofView_preferredEdge(
            self.name.bounds(),
            &self.name,
            NSRectEdge::MaxY,
        );
    }
}

/// A row of the box list: an icon and a name, highlighted under the pointer.
fn list_row(mtm: MainThreadMarker, title: &str, target: &Target, tag: isize) -> Retained<NSButton> {
    let row = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(sel!(fire:)),
            mtm,
        )
    };
    row.setBezelStyle(objc2_app_kit::NSBezelStyle::AccessoryBarAction);
    row.setShowsBorderOnlyWhileMouseInside(true);
    row.setAlignment(objc2_app_kit::NSTextAlignment::Left);
    row.setImagePosition(objc2_app_kit::NSCellImagePosition::ImageLeading);
    row.setTag(tag);
    row.setTranslatesAutoresizingMaskIntoConstraints(false);
    row.widthAnchor()
        .constraintEqualToConstant(ROW_WIDTH)
        .setActive(true);
    row.heightAnchor()
        .constraintEqualToConstant(ROW_HEIGHT)
        .setActive(true);
    row
}

/// The toolbar of the window that holds `ns_view`.
pub struct WindowToolbar {
    state: Rc<State>,
    modes: Retained<NSTabView>,
    status: Retained<NSTextField>,
    _delegate: Retained<Delegate>,
    _targets: Vec<Retained<Target>>,
}

impl WindowToolbar {
    /// `colors` and `symbols` are what the icon picker offers; picks come back as indexes into
    /// them.
    ///
    /// # Safety
    /// `ns_view` must point to a live `NSView` in a window, and this must run on the main
    /// thread.
    pub unsafe fn attach(
        ns_view: NonNull<c_void>,
        colors: &[&str],
        symbols: &[&str],
        avatar: Option<&Path>,
        initial: &str,
        on_event: impl Fn(ToolbarEvent) + 'static,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let view: &NSView = unsafe { ns_view.cast().as_ref() };
        let window = view.window()?;

        // The box: a colored tile with its glyph, the name with a chevron, and a grey status.
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
        let glyph = NSImageView::initWithFrame(
            NSImageView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(TILE, TILE)),
        );
        glyph.setImageScaling(NSImageScaling::ScaleNone);
        glyph.setContentTintColor(Some(&NSColor::whiteColor()));
        tile.addSubview(&glyph);

        let name =
            unsafe { NSButton::buttonWithTitle_target_action(&NSString::new(), None, None, mtm) };
        name.setBordered(false);
        name.setFont(Some(&NSFont::systemFontOfSize_weight(15., unsafe {
            objc2_app_kit::NSFontWeightSemibold
        })));
        if let Some(chevron) = small_symbol("chevron.down", "Boxes") {
            name.setImage(Some(&chevron));
            name.setImagePosition(objc2_app_kit::NSCellImagePosition::ImageTrailing);
        }
        name.setToolTip(Some(&NSString::from_str("Switch box")));
        let status = NSTextField::labelWithString(&NSString::new(), mtm);
        status.setFont(Some(&NSFont::systemFontOfSize(15.)));
        status.setTextColor(Some(&NSColor::secondaryLabelColor()));
        let stack = NSStackView::stackViewWithViews(
            &NSArray::from_retained_slice(&[
                tile.clone(),
                Retained::into_super(Retained::into_super(name.clone())),
                Retained::into_super(Retained::into_super(status.clone())),
            ]),
            mtm,
        );
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        stack.setSpacing(8.);

        let picker = NSPopover::new(mtm);
        picker.setBehavior(NSPopoverBehavior::Transient);
        let list = NSPopover::new(mtm);
        list.setBehavior(NSPopoverBehavior::Transient);

        // The picker's buttons report to a target made before the state that holds them.
        let picker_events: Rc<RefCell<Option<Rc<State>>>> = Rc::new(RefCell::new(None));
        let picker_target = Target::new(mtm, {
            let state = picker_events.clone();
            move |sender| {
                let Some(state) = state.borrow().clone() else {
                    return;
                };
                if let Some(tabs) = sender.downcast_ref::<NSSegmentedControl>() {
                    state.picker_view.show_tab(tabs.selectedSegment());
                    return;
                }
                let Some(button) = sender.downcast_ref::<NSButton>() else {
                    return;
                };
                let tag = button.tag();
                (state.on_event)(if tag >= GLYPH_TAG {
                    ToolbarEvent::Symbol((tag - GLYPH_TAG) as usize)
                } else {
                    ToolbarEvent::Color(tag.max(0) as usize)
                });
            }
        });
        let picker_view = Picker::new(mtm, colors, symbols, &picker_target);
        let controller =
            NSViewController::initWithNibName_bundle(NSViewController::alloc(mtm), None, None);
        picker.setContentSize(picker_view.view.frame().size);
        controller.setView(&picker_view.view);
        picker.setContentViewController(Some(&controller));

        let state = Rc::new(State {
            on_event: Box::new(on_event),
            boxes: RefCell::new(Vec::new()),
            current: Cell::new(None),
            tile: tile.clone(),
            glyph,
            name: name.clone(),
            picker,
            picker_view,
            list,
        });
        *picker_events.borrow_mut() = Some(state.clone());

        let tile_target = Target::new(mtm, {
            let state = state.clone();
            move |_| State::toggle(&state.picker, &state.tile)
        });
        let click = unsafe {
            NSClickGestureRecognizer::initWithTarget_action(
                NSClickGestureRecognizer::alloc(mtm),
                Some(&tile_target),
                Some(sel!(fire:)),
            )
        };
        tile.addGestureRecognizer(&click);
        let menu_target = Target::new(mtm, {
            let state = state.clone();
            move |sender| {
                let Some(row) = sender.downcast_ref::<NSButton>() else {
                    return;
                };
                unsafe { state.list.performClose(None) };
                (state.on_event)(match row.tag() {
                    NEW_BOX_TAG => ToolbarEvent::NewBox,
                    index => ToolbarEvent::OpenBox(index.max(0) as usize),
                });
            }
        });
        let name_target = Target::new(mtm, {
            let state = state.clone();
            let menu_target = menu_target.clone();
            move |_| state.show_boxes(&menu_target)
        });
        unsafe {
            name.setTarget(Some(&name_target));
            name.setAction(Some(sel!(fire:)));
        }

        let box_item = item(mtm, BOX, "Box");
        box_item.setView(Some(&stack));
        box_item.setBordered(false);

        // The add button.
        let add_target = Target::new(mtm, {
            let state = state.clone();
            move |_| (state.on_event)(ToolbarEvent::Add)
        });
        let add = item(mtm, ADD, "Add");
        add.setImage(symbol("plus", "Add files or folders").as_deref());
        add.setToolTip(Some(&NSString::from_str("Add files or folders")));
        unsafe {
            add.setTarget(Some(&add_target));
            add.setAction(Some(sel!(fire:)));
        }

        // Canvas and list: the tab strip of a real NSTabView, which is the control that lifts
        // the glass bubble while pressed. The strip alone goes in the toolbar; the tab view
        // stays out of the window and keeps driving it.
        let images = icon_images(&[("number", "Canvas"), ("list.bullet", "List")])?;
        let tabs = icon_tab_view(mtm, images.len(), &images, 0.);
        let strip = segments(&tabs)?;
        strip.removeFromSuperview();
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
                    decorate_strip(&strip, &images, 0.);
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

        let avatar_item = item(mtm, AVATAR, "Account");
        avatar_item.setView(Some(&account(mtm, avatar, initial)));

        let delegate: Retained<Delegate> = {
            let this = Delegate::alloc(mtm).set_ivars(DelegateIvars {
                items: vec![
                    (BOX, box_item),
                    (ADD, add),
                    (MODES, modes_item),
                    (AVATAR, avatar_item),
                ],
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
            _targets: vec![
                tile_target,
                menu_target,
                name_target,
                picker_target,
                add_target,
                modes_target,
            ],
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
        state
            .glyph
            .setImage(small_symbol(&label.symbol, &label.name).as_deref());
        if let Some(layer) = state.tile.layer() {
            layer.setBackgroundColor(Some(&box_color(&label.color).CGColor()));
        }
        state.picker_view.mark(&label.color, &label.symbol);
        self.state.name.setTitle(&NSString::from_str(&label.name));
        self.status.setStringValue(&NSString::from_str(status));
    }

    /// The boxes the name's menu offers, and which of them is open.
    pub fn set_boxes(&self, boxes: Vec<BoxLabel>, current: Option<usize>) {
        *self.state.boxes.borrow_mut() = boxes;
        self.state.current.set(current);
    }
}

fn item(mtm: MainThreadMarker, id: &str, label: &str) -> Retained<NSToolbarItem> {
    let item =
        NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), &NSString::from_str(id));
    item.setLabel(&NSString::from_str(label));
    item
}

/// The icon picker: a Color / Glyph switch over a grid of swatches or glyphs, like the one in
/// Shortcuts.
struct Picker {
    view: Retained<NSView>,
    colors: Vec<(String, Retained<NSButton>)>,
    glyphs: Vec<(String, Retained<NSButton>)>,
    color_grid: Retained<NSView>,
    glyph_grid: Retained<NSView>,
}

impl Picker {
    fn new(mtm: MainThreadMarker, colors: &[&str], symbols: &[&str], target: &Target) -> Self {
        let width = PICKER_PADDING * 2. + GLYPH_COLUMNS as f64 * GLYPH;
        let glyph_rows = symbols.len().div_ceil(GLYPH_COLUMNS);
        let grid_height = glyph_rows as f64 * GLYPH;
        let tabs_height = 24.;
        let height = PICKER_PADDING * 3. + tabs_height + grid_height;
        let view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(width, height)),
        );

        let tabs = unsafe {
            NSSegmentedControl::segmentedControlWithLabels_trackingMode_target_action(
                &NSArray::from_retained_slice(&[
                    NSString::from_str("Color"),
                    NSString::from_str("Glyph"),
                ]),
                NSSegmentSwitchTracking::SelectOne,
                Some(target),
                Some(sel!(fire:)),
                mtm,
            )
        };
        tabs.setSelectedSegment(0);
        tabs.setSegmentDistribution(objc2_app_kit::NSSegmentDistribution::FillEqually);
        tabs.setFrame(NSRect::new(
            NSPoint::new(PICKER_PADDING, height - PICKER_PADDING - tabs_height),
            NSSize::new(width - PICKER_PADDING * 2., tabs_height),
        ));
        view.addSubview(&tabs);

        let grid_frame = NSRect::new(
            NSPoint::new(PICKER_PADDING, PICKER_PADDING),
            NSSize::new(width - PICKER_PADDING * 2., grid_height),
        );
        let color_grid = NSView::initWithFrame(NSView::alloc(mtm), grid_frame);
        let glyph_grid = NSView::initWithFrame(NSView::alloc(mtm), grid_frame);
        glyph_grid.setHidden(true);
        view.addSubview(&color_grid);
        view.addSubview(&glyph_grid);

        // AppKit counts y from the bottom: rows go down from the top of the grid.
        let swatch_left = (grid_frame.size.width
            - SWATCH_COLUMNS as f64 * SWATCH
            - (SWATCH_COLUMNS - 1) as f64 * SWATCH_GAP)
            / 2.;
        let mut swatches = Vec::new();
        for (index, color) in colors.iter().enumerate() {
            let row = index / SWATCH_COLUMNS;
            let column = index % SWATCH_COLUMNS;
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::new(),
                    Some(target),
                    Some(sel!(fire:)),
                    mtm,
                )
            };
            button.setBordered(false);
            button.setTag(index as isize);
            button.setToolTip(Some(&NSString::from_str(color)));
            let swatch = NSView::initWithFrame(
                NSView::alloc(mtm),
                NSRect::new(NSPoint::new(4., 4.), NSSize::new(SWATCH - 8., SWATCH - 8.)),
            );
            swatch.setWantsLayer(true);
            if let Some(layer) = swatch.layer() {
                layer.setCornerRadius((SWATCH - 8.) / 2.);
                layer.setBackgroundColor(Some(&box_color(color).CGColor()));
            }
            button.addSubview(&swatch);
            button.setWantsLayer(true);
            if let Some(layer) = button.layer() {
                layer.setCornerRadius(SWATCH / 2.);
            }
            button.setFrame(NSRect::new(
                NSPoint::new(
                    swatch_left + column as f64 * (SWATCH + SWATCH_GAP),
                    grid_height - (row + 1) as f64 * (SWATCH + SWATCH_GAP),
                ),
                NSSize::new(SWATCH, SWATCH),
            ));
            color_grid.addSubview(&button);
            swatches.push((color.to_string(), button));
        }

        let mut glyphs = Vec::new();
        for (index, name) in symbols.iter().enumerate() {
            let Some(image) = symbol(name, name) else {
                continue;
            };
            let row = index / GLYPH_COLUMNS;
            let column = index % GLYPH_COLUMNS;
            let button = unsafe {
                NSButton::buttonWithImage_target_action(
                    &image,
                    Some(target),
                    Some(sel!(fire:)),
                    mtm,
                )
            };
            button.setBordered(false);
            button.setTag(GLYPH_TAG + index as isize);
            button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
            button.setWantsLayer(true);
            if let Some(layer) = button.layer() {
                layer.setCornerRadius(8.);
            }
            button.setFrame(NSRect::new(
                NSPoint::new(
                    column as f64 * GLYPH,
                    grid_height - (row + 1) as f64 * GLYPH,
                ),
                NSSize::new(GLYPH, GLYPH),
            ));
            glyph_grid.addSubview(&button);
            glyphs.push((name.to_string(), button));
        }
        Self {
            view,
            colors: swatches,
            glyphs,
            color_grid,
            glyph_grid,
        }
    }

    fn show_tab(&self, index: isize) {
        self.color_grid.setHidden(index != 0);
        self.glyph_grid.setHidden(index == 0);
    }

    /// Rings the box's color and highlights its glyph.
    fn mark(&self, color: &str, glyph: &str) {
        for (name, button) in &self.colors {
            if let Some(layer) = button.layer() {
                let selected = name == color;
                layer.setBorderWidth(if selected { 2.5 } else { 0. });
                layer.setBorderColor(Some(&box_color(name).CGColor()));
            }
        }
        for (name, button) in &self.glyphs {
            let selected = name == glyph;
            if let Some(layer) = button.layer() {
                let fill = NSColor::quaternaryLabelColor().CGColor();
                layer.setBackgroundColor(selected.then_some(&*fill));
            }
            button.setContentTintColor(Some(&*if selected {
                NSColor::labelColor()
            } else {
                NSColor::secondaryLabelColor()
            }));
        }
    }
}

/// The system color a box color name stands for.
fn box_color(name: &str) -> Retained<NSColor> {
    match name {
        "red" => NSColor::systemRedColor(),
        "orange" => NSColor::systemOrangeColor(),
        "yellow" => NSColor::systemYellowColor(),
        "green" => NSColor::systemGreenColor(),
        "mint" => NSColor::systemMintColor(),
        "teal" => NSColor::systemTealColor(),
        "indigo" => NSColor::systemIndigoColor(),
        "purple" => NSColor::systemPurpleColor(),
        "pink" => NSColor::systemPinkColor(),
        _ => NSColor::systemBlueColor(),
    }
}

/// An SF Symbol at the size of the box's tile.
fn small_symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
    let image = symbol(name, description)?;
    let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(12., unsafe {
        objc2_app_kit::NSFontWeightSemibold
    });
    image.imageWithSymbolConfiguration(&config)
}

/// An SF Symbol drawn in a box's color, for the box menu.
fn colored_symbol(name: &str, color: &str) -> Option<Retained<NSImage>> {
    let image = symbol(name, name)?;
    let config = NSImageSymbolConfiguration::configurationWithPaletteColors(
        &NSArray::from_retained_slice(&[box_color(color)]),
    );
    image.imageWithSymbolConfiguration(&config)
}

/// The account picture cut to a circle, or the user's initial as a button, for the toolbar
/// to put on glass.
fn account(mtm: MainThreadMarker, picture: Option<&Path>, initial: &str) -> Retained<NSView> {
    let image = picture.and_then(|path| {
        NSImage::initWithContentsOfFile(
            NSImage::alloc(),
            &NSString::from_str(&path.to_string_lossy()),
        )
    });
    match image {
        Some(image) => {
            let view = NSImageView::imageViewWithImage(&image, mtm);
            view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
            view.setFrame(NSRect::new(
                NSPoint::new(0., 0.),
                NSSize::new(AVATAR_SIZE, AVATAR_SIZE),
            ));
            view.setWantsLayer(true);
            if let Some(layer) = view.layer() {
                layer.setCornerRadius(AVATAR_SIZE / 2.);
                layer.setMasksToBounds(true);
            }
            Retained::into_super(Retained::into_super(view))
        }
        None => {
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(initial),
                    None,
                    None,
                    mtm,
                )
            };
            Retained::into_super(Retained::into_super(button))
        }
    }
}
