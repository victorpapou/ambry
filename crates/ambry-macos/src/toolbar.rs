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
    NSButton, NSColor, NSControlStateValueOff, NSControlStateValueOn, NSFont, NSImage,
    NSImageScaling, NSImageSymbolConfiguration, NSImageView, NSMenu, NSMenuItem, NSPopover,
    NSPopoverBehavior, NSStackView, NSTextField, NSToolbar, NSToolbarDelegate,
    NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem, NSToolbarItemGroup,
    NSToolbarItemGroupSelectionMode, NSToolbarItemIdentifier, NSToolbarSpaceItemIdentifier,
    NSUserInterfaceLayoutOrientation, NSView, NSViewController, NSWindowToolbarStyle,
};
use objc2_foundation::{NSArray, NSCopying, NSPoint, NSRect, NSRectEdge, NSSize, NSString};

use crate::native::{Target, symbol};

const BOX: &str = "box";
const ADD: &str = "add";
const MODES: &str = "modes";
const AVATAR: &str = "avatar";
/// Point size of the account picture inside its glass circle.
const AVATAR_SIZE: f64 = 28.;
/// The box's icon tile next to its name.
const TILE: f64 = 22.;
/// Swatches and glyphs in the icon picker.
const SWATCH: f64 = 24.;
const GLYPH: f64 = 34.;
const PICKER_COLUMNS: usize = 6;
const PICKER_PADDING: f64 = 14.;
/// Tags of the glyph buttons in the picker start here; colors use 0 up.
const GLYPH_TAG: isize = 100;
/// Tag of the "New Box" menu item.
const NEW_BOX_TAG: isize = -1;

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

/// What the toolbar's controls share: the box list for the menu and the icon picker.
struct State {
    on_event: Box<dyn Fn(ToolbarEvent)>,
    boxes: RefCell<Vec<BoxLabel>>,
    current: Cell<Option<usize>>,
    tile: Retained<NSButton>,
    name: Retained<NSButton>,
    popover: Retained<NSPopover>,
}

impl State {
    fn show_picker(&self) {
        if self.popover.isShown() {
            unsafe { self.popover.performClose(None) };
            return;
        }
        self.popover.showRelativeToRect_ofView_preferredEdge(
            self.tile.bounds(),
            &self.tile,
            NSRectEdge::MaxY,
        );
    }

    /// The box menu under the name, with a check on the open box and "New Box" at the end.
    fn show_boxes(&self, target: &Target) {
        let mtm = MainThreadMarker::from(target);
        let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Boxes"));
        for (index, label) in self.boxes.borrow().iter().enumerate() {
            let item = menu_item(mtm, &label.name, target, index as isize);
            if let Some(image) = colored_symbol(&label.symbol, &label.color) {
                item.setImage(Some(&image));
            }
            item.setState(if self.current.get() == Some(index) {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            menu.addItem(&item);
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        let new = menu_item(mtm, "New Box", target, NEW_BOX_TAG);
        if let Some(image) = symbol("plus", "New Box") {
            new.setImage(Some(&image));
        }
        menu.addItem(&new);

        let bounds = self.name.bounds();
        let y = if self.name.isFlipped() {
            bounds.size.height + 4.
        } else {
            -4.
        };
        menu.popUpMenuPositioningItem_atLocation_inView(
            None,
            NSPoint::new(0., y),
            Some(&self.name),
        );
    }
}

fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    target: &Target,
    tag: isize,
) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(sel!(fire:)),
            &NSString::from_str(""),
        )
    };
    unsafe { item.setTarget(Some(target)) };
    item.setTag(tag);
    item
}

/// The toolbar of the window that holds `ns_view`.
pub struct WindowToolbar {
    state: Rc<State>,
    modes: Retained<NSToolbarItemGroup>,
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

        // The box: icon tile, name with a chevron, and a grey status.
        let tile =
            unsafe { NSButton::buttonWithTitle_target_action(&NSString::new(), None, None, mtm) };
        tile.setBordered(false);
        tile.setImageScaling(NSImageScaling::ScaleNone);
        tile.setContentTintColor(Some(&NSColor::whiteColor()));
        tile.setWantsLayer(true);
        if let Some(layer) = tile.layer() {
            layer.setCornerRadius(6.);
        }
        tile.setFrameSize(NSSize::new(TILE, TILE));
        tile.setToolTip(Some(&NSString::from_str("Change icon")));
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
                Retained::into_super(Retained::into_super(tile.clone())),
                Retained::into_super(Retained::into_super(name.clone())),
                Retained::into_super(Retained::into_super(status.clone())),
            ]),
            mtm,
        );
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        stack.setSpacing(8.);

        let popover = NSPopover::new(mtm);
        popover.setBehavior(NSPopoverBehavior::Transient);

        let state = Rc::new(State {
            on_event: Box::new(on_event),
            boxes: RefCell::new(Vec::new()),
            current: Cell::new(None),
            tile: tile.clone(),
            name: name.clone(),
            popover: popover.clone(),
        });

        let tile_target = Target::new(mtm, {
            let state = state.clone();
            move |_| state.show_picker()
        });
        let menu_target = Target::new(mtm, {
            let state = state.clone();
            move |sender| {
                let Some(item) = sender.downcast_ref::<NSMenuItem>() else {
                    return;
                };
                (state.on_event)(match item.tag() {
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
            tile.setTarget(Some(&tile_target));
            tile.setAction(Some(sel!(fire:)));
            name.setTarget(Some(&name_target));
            name.setAction(Some(sel!(fire:)));
        }
        let picker_target = Target::new(mtm, {
            let state = state.clone();
            move |sender| {
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
        let picker = icon_picker(mtm, colors, symbols, &picker_target);
        let controller =
            NSViewController::initWithNibName_bundle(NSViewController::alloc(mtm), None, None);
        popover.setContentSize(picker.frame().size);
        controller.setView(&picker);
        popover.setContentViewController(Some(&controller));

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

        // Canvas and list.
        let modes_target = Target::new(mtm, {
            let state = state.clone();
            move |sender| {
                if let Some(group) = sender.downcast_ref::<NSToolbarItemGroup>() {
                    if let Ok(index) = usize::try_from(group.selectedIndex()) {
                        (state.on_event)(ToolbarEvent::Mode(index));
                    }
                }
            }
        });
        let images: Vec<Retained<NSImage>> = [("number", "Canvas"), ("list.bullet", "List")]
            .iter()
            .filter_map(|(name, description)| symbol(name, description))
            .collect();
        let modes = unsafe {
            NSToolbarItemGroup::groupWithItemIdentifier_images_selectionMode_labels_target_action(
                &NSString::from_str(MODES),
                &NSArray::from_retained_slice(&images),
                NSToolbarItemGroupSelectionMode::SelectOne,
                Some(&NSArray::from_retained_slice(&[
                    NSString::from_str("Canvas"),
                    NSString::from_str("List"),
                ])),
                Some(&modes_target),
                Some(sel!(fire:)),
                mtm,
            )
        };
        modes.setLabel(&NSString::from_str("View"));
        modes.setSelectedIndex(0);

        let avatar_item = item(mtm, AVATAR, "Account");
        avatar_item.setView(Some(&account(mtm, avatar, initial)));

        let delegate: Retained<Delegate> = {
            let this = Delegate::alloc(mtm).set_ivars(DelegateIvars {
                items: vec![
                    (BOX, box_item),
                    (ADD, add),
                    (MODES, Retained::into_super(modes.clone())),
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
        window.setToolbarStyle(NSWindowToolbarStyle::Unified);
        window.setToolbar(Some(&toolbar));
        Some(Self {
            state,
            modes,
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
        if self.modes.selectedIndex() != index as isize {
            self.modes.setSelectedIndex(index as isize);
        }
    }

    /// Shows the open box: its icon, name and a status such as "3 items".
    pub fn set_box(&self, label: &BoxLabel, status: &str) {
        let tile = &self.state.tile;
        tile.setImage(small_symbol(&label.symbol, &label.name).as_deref());
        if let Some(layer) = tile.layer() {
            layer.setBackgroundColor(Some(&box_color(&label.color).CGColor()));
        }
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

/// Rows of color swatches over a grid of glyphs, like the icon picker in Shortcuts.
fn icon_picker(
    mtm: MainThreadMarker,
    colors: &[&str],
    symbols: &[&str],
    target: &Target,
) -> Retained<NSView> {
    let columns = PICKER_COLUMNS;
    let color_rows = colors.len().div_ceil(columns * 2 - 2).max(1);
    let colors_per_row = colors.len().div_ceil(color_rows).max(1);
    let glyph_rows = symbols.len().div_ceil(columns);
    let width = PICKER_PADDING * 2. + columns as f64 * GLYPH;
    let color_height = color_rows as f64 * (SWATCH + 8.);
    let height = PICKER_PADDING * 3. + color_height + glyph_rows as f64 * GLYPH;
    let view = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0., 0.), NSSize::new(width, height)),
    );

    // AppKit counts y from the bottom: the swatches go on top.
    let swatch_step = (width - PICKER_PADDING * 2. - SWATCH) / (colors_per_row.max(2) - 1) as f64;
    for (index, color) in colors.iter().enumerate() {
        let row = index / colors_per_row;
        let column = index % colors_per_row;
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
        button.setWantsLayer(true);
        if let Some(layer) = button.layer() {
            layer.setCornerRadius(SWATCH / 2.);
            layer.setBackgroundColor(Some(&box_color(color).CGColor()));
        }
        button.setFrame(NSRect::new(
            NSPoint::new(
                PICKER_PADDING + column as f64 * swatch_step,
                height - PICKER_PADDING - SWATCH - row as f64 * (SWATCH + 8.),
            ),
            NSSize::new(SWATCH, SWATCH),
        ));
        view.addSubview(&button);
    }

    let glyphs_top = height - PICKER_PADDING * 2. - color_height;
    for (index, name) in symbols.iter().enumerate() {
        let Some(image) = symbol(name, name) else {
            continue;
        };
        let row = index / columns;
        let column = index % columns;
        let button = unsafe {
            NSButton::buttonWithImage_target_action(&image, Some(target), Some(sel!(fire:)), mtm)
        };
        button.setBordered(false);
        button.setTag(GLYPH_TAG + index as isize);
        button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        button.setFrame(NSRect::new(
            NSPoint::new(
                PICKER_PADDING + column as f64 * GLYPH,
                glyphs_top - (row + 1) as f64 * GLYPH,
            ),
            NSSize::new(GLYPH, GLYPH),
        ));
        view.addSubview(&button);
    }
    view
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
