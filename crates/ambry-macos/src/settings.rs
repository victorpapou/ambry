//! Ambry's Settings window, laid out like the settings of Apple's own apps: a toolbar of
//! tabs on top, and under it a form of right-aligned labels and their controls. Choices
//! live in the app's user defaults; keys and tokens live in the keychain.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSAppearance, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication,
    NSBackingStoreType, NSBitmapImageFileType, NSBitmapImageRep, NSButton, NSColor,
    NSCompositingOperation, NSControlStateValueOn, NSDeviceRGBColorSpace, NSFont,
    NSGraphicsContext, NSGridCell, NSGridCellPlacement, NSGridRowAlignment, NSGridView, NSImage,
    NSImageScaling, NSImageView, NSLineBreakMode, NSOpenPanel, NSPopUpButton, NSSecureTextField,
    NSTextField, NSToolbar, NSToolbarDelegate, NSToolbarItem, NSToolbarItemIdentifier, NSView,
    NSWindow, NSWindowStyleMask, NSWindowToolbarStyle, NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSPoint, NSRect, NSSize, NSString, NSURL, NSUserDefaults,
};

use crate::native::{Target, symbol};

/// Width of every pane, as Apple's settings windows keep one width across their tabs.
const WIDTH: f64 = 520.;
const PADDING: f64 = 24.;
const FIELD_WIDTH: f64 = 260.;
const AVATAR: f64 = 64.;
const KEYCHAIN_SERVICE: &str = "dev.ambry.Ambry";

const APPEARANCE: &str = "AmbryAppearance";
const OPEN_IN_LIST: &str = "AmbryOpenInList";
const HIDE_ITEM_COUNT: &str = "AmbryHideItemCount";
const USER_NAME: &str = "AmbryUserName";
const USER_EMAIL: &str = "AmbryUserEmail";
const MODEL: &str = "AmbryModel";
const ANTHROPIC_KEY: &str = "anthropic-api-key";
const TELEGRAM_TOKEN: &str = "telegram-bot-token";

/// The models offered, by their API names.
const MODELS: [(&str, &str); 3] = [
    ("Claude Opus 5.5", "claude-opus-5-5"),
    ("Claude Sonnet 5.5", "claude-sonnet-5-5"),
    ("Claude Haiku 5.5", "claude-haiku-5-5"),
];

/// The services Ambry will connect to, shown before their connections exist.
const SERVICES: [(&str, &str); 4] = [
    ("Slack", "number"),
    ("Figma", "pencil.and.outline"),
    ("Notion", "doc.text"),
    ("GitHub", "chevron.left.forwardslash.chevron.right"),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pane {
    General,
    Account,
    Ai,
    Connections,
}

impl Pane {
    const ALL: [Pane; 4] = [Pane::General, Pane::Account, Pane::Ai, Pane::Connections];

    fn id(self) -> &'static str {
        match self {
            Pane::General => "general",
            Pane::Account => "account",
            Pane::Ai => "ai",
            Pane::Connections => "connections",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Pane::General => "General",
            Pane::Account => "Account",
            Pane::Ai => "AI",
            Pane::Connections => "Connections",
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Pane::General => "gearshape",
            Pane::Account => "person.crop.circle",
            Pane::Ai => "sparkles",
            Pane::Connections => "puzzlepiece.extension",
        }
    }
}

/// What changed in Settings that the app's windows show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsEvent {
    /// The name or the picture.
    Account,
    /// What the toolbar shows next to the box's name.
    ItemCount,
}

struct Setup {
    avatar: PathBuf,
    boxes: PathBuf,
    on_change: Rc<dyn Fn(SettingsEvent)>,
}

struct Open {
    window: Retained<NSWindow>,
    _toolbar: Retained<ToolbarItems>,
    /// The tab targets, which live as long as the window.
    _tabs: Vec<Retained<Target>>,
    /// The targets of the pane on screen, let go when another pane replaces it.
    pane: RefCell<Vec<Retained<Target>>>,
}

thread_local! {
    static SETUP: RefCell<Option<Rc<Setup>>> = const { RefCell::new(None) };
    static OPEN: RefCell<Option<Rc<Open>>> = const { RefCell::new(None) };
}

/// Tells Settings where the account picture and the boxes live, and who hears changes.
pub fn configure(avatar: PathBuf, boxes: PathBuf, on_change: impl Fn(SettingsEvent) + 'static) {
    SETUP.with_borrow_mut(|setup| {
        *setup = Some(Rc::new(Setup {
            avatar,
            boxes,
            on_change: Rc::new(on_change),
        }))
    });
}

/// The name to show for the user: the one set in Settings, or the Mac account's.
pub fn user_name() -> String {
    string(USER_NAME)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| objc2_foundation::NSFullUserName().to_string())
}

/// Whether boxes open in the list rather than on the canvas.
pub fn opens_in_list() -> bool {
    defaults().boolForKey(&NSString::from_str(OPEN_IN_LIST))
}

/// Whether the toolbar shows how many items the box holds.
pub fn shows_item_count() -> bool {
    !defaults().boolForKey(&NSString::from_str(HIDE_ITEM_COUNT))
}

/// Puts the app in the appearance chosen in Settings.
pub fn apply_appearance() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let name = match defaults().integerForKey(&NSString::from_str(APPEARANCE)) {
        1 => Some(unsafe { NSAppearanceNameAqua }),
        2 => Some(unsafe { NSAppearanceNameDarkAqua }),
        _ => None,
    };
    let appearance = name.and_then(NSAppearance::appearanceNamed);
    NSApplication::sharedApplication(mtm).setAppearance(appearance.as_deref());
}

/// Opens the Settings window on `pane`, or brings it forward.
pub fn open(pane: Pane) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let open = OPEN.with_borrow(|open| open.clone()).unwrap_or_else(|| {
        let open = Rc::new(build(mtm));
        OPEN.with_borrow_mut(|slot| *slot = Some(open.clone()));
        open.window.center();
        open
    });
    show_pane(mtm, &open, pane, false);
    NSApplication::sharedApplication(mtm).activate();
    open.window.makeKeyAndOrderFront(None);
}

struct ToolbarIvars {
    items: Vec<Retained<NSToolbarItem>>,
}

define_class!(
    // Hands the Settings toolbar its tabs.
    #[unsafe(super(objc2::runtime::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbrySettingsToolbar"]
    #[ivars = ToolbarIvars]
    struct ToolbarItems;

    unsafe impl NSObjectProtocol for ToolbarItems {}

    unsafe impl NSToolbarDelegate for ToolbarItems {
        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item(
            &self,
            _toolbar: &NSToolbar,
            id: &NSToolbarItemIdentifier,
            _flag: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            self.ivars()
                .items
                .iter()
                .find(|item| &*item.itemIdentifier() == id)
                .cloned()
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn default_ids(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            pane_ids()
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed_ids(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            pane_ids()
        }

        #[unsafe(method_id(toolbarSelectableItemIdentifiers:))]
        fn selectable_ids(
            &self,
            _toolbar: &NSToolbar,
        ) -> Retained<NSArray<NSToolbarItemIdentifier>> {
            pane_ids()
        }
    }
);

fn pane_ids() -> Retained<NSArray<NSToolbarItemIdentifier>> {
    let ids: Vec<Retained<NSString>> = Pane::ALL
        .iter()
        .map(|pane| NSString::from_str(pane.id()))
        .collect();
    NSArray::from_retained_slice(&ids)
}

fn build(mtm: MainThreadMarker) -> Open {
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, 200.)),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setToolbarStyle(NSWindowToolbarStyle::Preference);

    let mut tabs = Vec::new();
    let items = Pane::ALL
        .iter()
        .map(|&pane| {
            let item = NSToolbarItem::initWithItemIdentifier(
                NSToolbarItem::alloc(mtm),
                &NSString::from_str(pane.id()),
            );
            item.setLabel(&NSString::from_str(pane.title()));
            item.setImage(symbol(pane.symbol(), pane.title()).as_deref());
            let target = Target::new(mtm, move |_| {
                let open = OPEN.with_borrow(|open| open.clone());
                if let (Some(open), Some(mtm)) = (open, MainThreadMarker::new()) {
                    show_pane(mtm, &open, pane, true);
                }
            });
            unsafe {
                item.setTarget(Some(&target));
                item.setAction(Some(sel!(fire:)));
            }
            tabs.push(target);
            item
        })
        .collect();
    let toolbar_items: Retained<ToolbarItems> = {
        let this = ToolbarItems::alloc(mtm).set_ivars(ToolbarIvars { items });
        unsafe { msg_send![super(this), init] }
    };
    let toolbar =
        NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), &NSString::from_str("ambry.settings"));
    toolbar.setDelegate(Some(ProtocolObject::from_ref(&*toolbar_items)));
    toolbar.setAllowsUserCustomization(false);
    window.setToolbar(Some(&toolbar));
    Open {
        window,
        _toolbar: toolbar_items,
        _tabs: tabs,
        pane: RefCell::new(Vec::new()),
    }
}

fn show_pane(mtm: MainThreadMarker, open: &Open, pane: Pane, animate: bool) {
    let window = &open.window;
    if let Some(toolbar) = window.toolbar() {
        toolbar.setSelectedItemIdentifier(Some(&NSString::from_str(pane.id())));
    }
    window.setTitle(&NSString::from_str(pane.title()));
    let mut targets = Vec::new();
    let rows = match pane {
        Pane::General => general(mtm, &mut targets),
        Pane::Account => account(mtm, &mut targets),
        Pane::Ai => ai(mtm, &mut targets),
        Pane::Connections => connections(mtm, &mut targets),
    };
    let content = form(mtm, rows);
    let size = content.fittingSize();
    let size = NSSize::new(WIDTH, size.height.ceil());

    // Grow or shrink from the top, as settings windows do between tabs.
    let old = window.frame();
    let mut frame = window.frameRectForContentRect(NSRect::new(NSPoint::ZERO, size));
    frame.origin = NSPoint::new(
        old.origin.x,
        old.origin.y + old.size.height - frame.size.height,
    );
    let blank = NSView::initWithFrame(NSView::alloc(mtm), NSRect::ZERO);
    window.setContentView(Some(&blank));
    window.setFrame_display_animate(frame, true, animate && window.isVisible());
    window.setContentView(Some(&content));
    *open.pane.borrow_mut() = targets;
}

/// One line of the form: a right-aligned label, or none, and its control.
type Row = (Option<&'static str>, Retained<NSView>);

/// The form of `rows`, padded and at the panes' width.
fn form(mtm: MainThreadMarker, rows: Vec<Row>) -> Retained<NSView> {
    let cells: Vec<Retained<NSArray<NSView>>> = rows
        .into_iter()
        .map(|(label, control)| {
            let label: Retained<NSView> = match label {
                Some(text) => {
                    let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
                    Retained::into_super(Retained::into_super(field))
                }
                None => NSGridCell::emptyContentView(mtm),
            };
            NSArray::from_retained_slice(&[label, control])
        })
        .collect();
    let grid = NSGridView::gridViewWithViews(&NSArray::from_retained_slice(&cells), mtm);
    grid.setRowSpacing(10.);
    grid.setColumnSpacing(10.);
    grid.setRowAlignment(NSGridRowAlignment::FirstBaseline);
    if grid.numberOfColumns() > 0 {
        grid.columnAtIndex(0)
            .setXPlacement(NSGridCellPlacement::Trailing);
        grid.columnAtIndex(1)
            .setXPlacement(NSGridCellPlacement::Leading);
    }
    grid.setTranslatesAutoresizingMaskIntoConstraints(false);

    let content = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, 100.)),
    );
    content.addSubview(&grid);
    content
        .widthAnchor()
        .constraintEqualToConstant(WIDTH)
        .setActive(true);
    grid.topAnchor()
        .constraintEqualToAnchor_constant(&content.topAnchor(), PADDING)
        .setActive(true);
    grid.bottomAnchor()
        .constraintEqualToAnchor_constant(&content.bottomAnchor(), -PADDING)
        .setActive(true);
    grid.centerXAnchor()
        .constraintEqualToAnchor(&content.centerXAnchor())
        .setActive(true);
    content
}

fn general(mtm: MainThreadMarker, targets: &mut Vec<Retained<Target>>) -> Vec<Row> {
    let appearance = popup(
        mtm,
        &["System", "Light", "Dark"],
        defaults().integerForKey(&NSString::from_str(APPEARANCE)) as isize,
        targets,
        |index| {
            defaults().setInteger_forKey(index as isize, &NSString::from_str(APPEARANCE));
            apply_appearance();
        },
    );
    let open_in = popup(
        mtm,
        &["Canvas", "List"],
        opens_in_list() as isize,
        targets,
        |index| defaults().setBool_forKey(index == 1, &NSString::from_str(OPEN_IN_LIST)),
    );
    let count = checkbox(
        mtm,
        "Show the number of items next to the box’s name",
        shows_item_count(),
        targets,
        |on| {
            defaults().setBool_forKey(!on, &NSString::from_str(HIDE_ITEM_COUNT));
            changed(SettingsEvent::ItemCount);
        },
    );
    let boxes = setup().map(|setup| setup.boxes.clone()).unwrap_or_default();
    let folder = caption(mtm, &boxes.to_string_lossy());
    folder.setTextColor(Some(&NSColor::labelColor()));
    folder.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    width(&folder, FIELD_WIDTH);
    let reveal = push_button(mtm, "Show in Finder", targets, move || {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&boxes.to_string_lossy()));
        NSWorkspace::sharedWorkspace()
            .activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
    });
    vec![
        (Some("Appearance:"), appearance),
        (Some("Open boxes in:"), open_in),
        (Some("Toolbar:"), count),
        (Some("Boxes folder:"), folder.into_super().into_super()),
        (None, reveal),
    ]
}

fn account(mtm: MainThreadMarker, targets: &mut Vec<Retained<Target>>) -> Vec<Row> {
    let picture = NSImageView::initWithFrame(
        NSImageView::alloc(mtm),
        NSRect::new(NSPoint::ZERO, NSSize::new(AVATAR, AVATAR)),
    );
    picture.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    picture.setWantsLayer(true);
    if let Some(layer) = picture.layer() {
        layer.setCornerRadius(AVATAR / 2.);
        layer.setMasksToBounds(true);
    }
    width(&picture, AVATAR);
    picture
        .heightAnchor()
        .constraintEqualToConstant(AVATAR)
        .setActive(true);
    show_avatar(&picture);

    let choose = push_button(mtm, "Choose…", targets, {
        let picture = picture.clone();
        move || {
            if choose_avatar() {
                show_avatar(&picture);
                changed(SettingsEvent::Account);
            }
        }
    });
    let remove = push_button(mtm, "Remove", targets, {
        let picture = picture.clone();
        move || {
            if let Some(setup) = setup() {
                let _ = std::fs::remove_file(&setup.avatar);
            }
            show_avatar(&picture);
            changed(SettingsEvent::Account);
        }
    });
    let buttons = row(mtm, &[choose, remove]);

    let name = text_field(
        mtm,
        &string(USER_NAME).unwrap_or_default(),
        &objc2_foundation::NSFullUserName().to_string(),
        targets,
        |text| {
            set_string(USER_NAME, text);
            changed(SettingsEvent::Account);
        },
    );
    let email = text_field(
        mtm,
        &string(USER_EMAIL).unwrap_or_default(),
        "name@example.com",
        targets,
        |text| set_string(USER_EMAIL, text),
    );
    vec![
        (Some("Picture:"), picture.into_super().into_super()),
        (None, buttons),
        (Some("Name:"), name),
        (Some("Email:"), email),
        (
            None,
            caption(
                mtm,
                "Your name and picture show in the toolbar of every box.",
            )
            .into_super()
            .into_super(),
        ),
    ]
}

fn ai(mtm: MainThreadMarker, targets: &mut Vec<Retained<Target>>) -> Vec<Row> {
    let provider = popup(mtm, &["Anthropic"], 0, targets, |_| {});
    let key = secret_field(mtm, ANTHROPIC_KEY, "sk-ant-…", targets);
    let model_names: Vec<&str> = MODELS.iter().map(|(name, _)| *name).collect();
    let current = string(MODEL)
        .and_then(|id| MODELS.iter().position(|(_, model)| *model == id))
        .unwrap_or(0);
    let model = popup(mtm, &model_names, current as isize, targets, |index| {
        if let Some((_, id)) = MODELS.get(index) {
            set_string(MODEL, id);
        }
    });
    vec![
        (Some("Provider:"), provider),
        (Some("API key:"), key),
        (
            None,
            caption(
                mtm,
                "Kept in your Mac’s keychain and sent only to Anthropic.",
            )
            .into_super()
            .into_super(),
        ),
        (Some("Model:"), model),
    ]
}

fn connections(mtm: MainThreadMarker, targets: &mut Vec<Retained<Target>>) -> Vec<Row> {
    let token = secret_field(mtm, TELEGRAM_TOKEN, "Bot token from @BotFather", targets);
    let mut rows: Vec<Row> = vec![
        (Some("Telegram bot:"), token),
        (
            None,
            caption(
                mtm,
                "Kept in your keychain. Forwarding messages into boxes comes in a later build.",
            )
            .into_super()
            .into_super(),
        ),
    ];
    for (index, (name, glyph)) in SERVICES.iter().enumerate() {
        let icon = NSImageView::initWithFrame(NSImageView::alloc(mtm), NSRect::ZERO);
        icon.setImage(symbol(glyph, name).as_deref());
        icon.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        let title = NSTextField::labelWithString(&NSString::from_str(name), mtm);
        width(&title, 70.);
        let connect = unsafe {
            NSButton::buttonWithTitle_target_action(&NSString::from_str("Connect"), None, None, mtm)
        };
        connect.setEnabled(false);
        let soon = caption(mtm, "Coming soon");
        let line = row(
            mtm,
            &[
                icon.into_super().into_super(),
                title.into_super().into_super(),
                connect.into_super().into_super(),
                soon.into_super().into_super(),
            ],
        );
        rows.push((if index == 0 { Some("Services:") } else { None }, line));
    }
    rows
}

fn popup(
    mtm: MainThreadMarker,
    titles: &[&str],
    selected: isize,
    targets: &mut Vec<Retained<Target>>,
    on_select: impl Fn(usize) + 'static,
) -> Retained<NSView> {
    let popup =
        NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(mtm), NSRect::ZERO, false);
    for title in titles {
        popup.addItemWithTitle(&NSString::from_str(title));
    }
    popup.selectItemAtIndex(selected.clamp(0, titles.len() as isize - 1));
    let target = Target::new(mtm, move |sender| {
        if let Some(popup) = sender.downcast_ref::<NSPopUpButton>() {
            on_select(popup.indexOfSelectedItem().max(0) as usize);
        }
    });
    unsafe {
        popup.setTarget(Some(&target));
        popup.setAction(Some(sel!(fire:)));
    }
    targets.push(target);
    popup.into_super().into_super().into_super()
}

fn checkbox(
    mtm: MainThreadMarker,
    title: &str,
    on: bool,
    targets: &mut Vec<Retained<Target>>,
    on_toggle: impl Fn(bool) + 'static,
) -> Retained<NSView> {
    let target = Target::new(mtm, move |sender| {
        if let Some(button) = sender.downcast_ref::<NSButton>() {
            on_toggle(button.state() == NSControlStateValueOn);
        }
    });
    let checkbox = unsafe {
        NSButton::checkboxWithTitle_target_action(
            &NSString::from_str(title),
            Some(&target),
            Some(sel!(fire:)),
            mtm,
        )
    };
    checkbox.setState(if on { NSControlStateValueOn } else { 0 });
    targets.push(target);
    checkbox.into_super().into_super()
}

fn push_button(
    mtm: MainThreadMarker,
    title: &str,
    targets: &mut Vec<Retained<Target>>,
    on_press: impl Fn() + 'static,
) -> Retained<NSView> {
    let target = Target::new(mtm, move |_| on_press());
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(&target),
            Some(sel!(fire:)),
            mtm,
        )
    };
    targets.push(target);
    button.into_super().into_super()
}

fn text_field(
    mtm: MainThreadMarker,
    value: &str,
    placeholder: &str,
    targets: &mut Vec<Retained<Target>>,
    on_change: impl Fn(&str) + 'static,
) -> Retained<NSView> {
    let field = NSTextField::textFieldWithString(&NSString::from_str(value), mtm);
    field.setPlaceholderString(Some(&NSString::from_str(placeholder)));
    watch(mtm, &field, targets, on_change);
    width(&field, FIELD_WIDTH);
    field.into_super().into_super()
}

/// A field for a key or token kept in the keychain under `account`.
fn secret_field(
    mtm: MainThreadMarker,
    account: &'static str,
    placeholder: &str,
    targets: &mut Vec<Retained<Target>>,
) -> Retained<NSView> {
    let field = NSSecureTextField::initWithFrame(NSSecureTextField::alloc(mtm), NSRect::ZERO);
    field.setStringValue(&NSString::from_str(&secret(account).unwrap_or_default()));
    field.setPlaceholderString(Some(&NSString::from_str(placeholder)));
    watch(mtm, &field, targets, move |text| set_secret(account, text));
    width(&field, FIELD_WIDTH);
    Retained::into_super(Retained::into_super(Retained::into_super(field)))
}

/// Saves `field` when the user presses Return or leaves it.
fn watch(
    mtm: MainThreadMarker,
    field: &NSTextField,
    targets: &mut Vec<Retained<Target>>,
    on_change: impl Fn(&str) + 'static,
) {
    let target = Target::new(mtm, move |sender| {
        if let Some(field) = sender.downcast_ref::<NSTextField>() {
            on_change(field.stringValue().to_string().trim());
        }
    });
    if let Some(cell) = field.cell() {
        cell.setSendsActionOnEndEditing(true);
    }
    unsafe {
        field.setTarget(Some(&target));
        field.setAction(Some(sel!(fire:)));
    }
    targets.push(target);
}

fn caption(mtm: MainThreadMarker, text: &str) -> Retained<NSTextField> {
    let label = NSTextField::wrappingLabelWithString(&NSString::from_str(text), mtm);
    label.setFont(Some(&NSFont::systemFontOfSize(
        NSFont::smallSystemFontSize(),
    )));
    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    label.setPreferredMaxLayoutWidth(FIELD_WIDTH + 40.);
    label
}

/// `views` side by side.
fn row(mtm: MainThreadMarker, views: &[Retained<NSView>]) -> Retained<NSView> {
    let stack =
        objc2_app_kit::NSStackView::stackViewWithViews(&NSArray::from_retained_slice(views), mtm);
    stack.setSpacing(8.);
    stack.into_super()
}

fn width(view: &NSView, points: f64) {
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    view.widthAnchor()
        .constraintEqualToConstant(points)
        .setActive(true);
}

fn show_avatar(view: &NSImageView) {
    let image = setup()
        .filter(|setup| setup.avatar.exists())
        .and_then(|setup| load_image(&setup.avatar));
    match image {
        Some(image) => {
            view.setImage(Some(&image));
            view.setContentTintColor(None);
        }
        None => {
            view.setImage(symbol("person.crop.circle.fill", "No picture").as_deref());
            view.setContentTintColor(Some(&NSColor::tertiaryLabelColor()));
        }
    }
}

/// Asks for a picture and saves it, cut to a square, as the account picture.
fn choose_avatar() -> bool {
    let (Some(mtm), Some(setup)) = (MainThreadMarker::new(), setup()) else {
        return false;
    };
    let panel = NSOpenPanel::openPanel(mtm);
    panel.setCanChooseFiles(true);
    panel.setCanChooseDirectories(false);
    panel.setAllowsMultipleSelection(false);
    panel.setMessage(Some(&NSString::from_str(
        "Choose a picture for your account",
    )));
    if panel.runModal() != 1 {
        return false;
    }
    let Some(path) = panel.URL().and_then(|url| url.path()) else {
        return false;
    };
    let Some(image) = load_image(Path::new(&path.to_string())) else {
        return false;
    };
    save_square_png(&image, &setup.avatar, 256)
}

fn load_image(path: &Path) -> Option<Retained<NSImage>> {
    NSImage::initWithContentsOfFile(
        NSImage::alloc(),
        &NSString::from_str(&path.to_string_lossy()),
    )
}

/// Writes `image` filling a `pixels` square, cut from its middle, as a PNG at `path`.
fn save_square_png(image: &NSImage, path: &Path, pixels: isize) -> bool {
    let size = image.size();
    if size.width <= 0. || size.height <= 0. {
        return false;
    }
    let Some(rep) = (unsafe {
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
    }) else {
        return false;
    };
    let Some(context) = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep) else {
        return false;
    };
    let side = pixels as f64;
    let scale = (side / size.width).max(side / size.height);
    let drawn = NSSize::new(size.width * scale, size.height * scale);
    NSGraphicsContext::saveGraphicsState_class();
    NSGraphicsContext::setCurrentContext(Some(&context));
    image.drawInRect_fromRect_operation_fraction(
        NSRect::new(
            NSPoint::new((side - drawn.width) / 2., (side - drawn.height) / 2.),
            drawn,
        ),
        NSRect::ZERO,
        NSCompositingOperation::Copy,
        1.,
    );
    context.flushGraphics();
    NSGraphicsContext::restoreGraphicsState_class();
    let Some(png) = (unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }) else {
        return false;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, png.to_vec()).is_ok()
}

fn setup() -> Option<Rc<Setup>> {
    SETUP.with_borrow(|setup| setup.clone())
}

fn changed(event: SettingsEvent) {
    if let Some(setup) = setup() {
        (setup.on_change)(event);
    }
}

fn defaults() -> Retained<NSUserDefaults> {
    NSUserDefaults::standardUserDefaults()
}

fn string(key: &str) -> Option<String> {
    defaults()
        .stringForKey(&NSString::from_str(key))
        .map(|value| value.to_string())
}

fn set_string(key: &str, value: &str) {
    let key = NSString::from_str(key);
    if value.is_empty() {
        defaults().removeObjectForKey(&key);
    } else {
        let value: &AnyObject = &NSString::from_str(value);
        unsafe { defaults().setObject_forKey(Some(value), &key) };
    }
}

fn secret(account: &str) -> Option<String> {
    security_framework::passwords::get_generic_password(KEYCHAIN_SERVICE, account)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

fn set_secret(account: &str, value: &str) {
    if secret(account).as_deref() == Some(value) {
        return;
    }
    let result = if value.is_empty() {
        security_framework::passwords::delete_generic_password(KEYCHAIN_SERVICE, account)
    } else {
        security_framework::passwords::set_generic_password(
            KEYCHAIN_SERVICE,
            account,
            value.as_bytes(),
        )
    };
    if let Err(err) = result {
        eprintln!("could not save {account} in the keychain: {err}");
    }
}
