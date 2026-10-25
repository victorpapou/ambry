//! Finder's column view: AppKit's own NSBrowser. The first column lists the box's items, each
//! folder opens in a column to its right, and dragging over rows picks several, all as AppKit
//! does it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBezierPath, NSBrowser, NSBrowserCell, NSCell, NSColor, NSCompositingOperation, NSFont,
    NSFontAttributeName, NSForegroundColorAttributeName, NSImage, NSImageScaling,
    NSImageView, NSLineBreakMode,
    NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSScrollView, NSStackView,
    NSStringDrawing, NSTextField, NSView, NSViewController, NSWorkspace,
};
use objc2_foundation::{NSArray, NSDictionary, NSIndexSet, NSPoint, NSRect, NSSize, NSString};

use crate::native::{NativeView, Target};

/// The width each column opens at.
const COLUMN_WIDTH: f64 = 240.;
/// The most entries one folder's column lists.
const LIMIT: usize = 500;

/// How far its highlight keeps from the column's sides, and its corners:
/// Finder's column view on macOS 26.
const ROW_INSET: f64 = 10.;
/// Height of a row. A browser with a matrix delegate refuses `setRowHeight:`, so each
/// column's matrix is given it.
const ROW_HEIGHT: f64 = 24.;
/// The highlight leaves this much clear above and below it, so neighbours do not touch.
const ROW_GAP: f64 = 1.;
const ROW_RADIUS: f64 = 6.;
const FONT_SIZE: f64 = 13.;
/// The icon, how far it sits inside the highlight, and the gap from it to the name.
const ICON: f64 = 16.;
const ICON_INSET: f64 = 7.;
const ICON_GAP: f64 = 6.;
/// Room the browser's chevron after a folder takes at the row's end.
const CHEVRON_ROOM: f64 = 16.;
/// Room above a column's first row and below its last, as Finder leaves.
const COLUMN_PAD: f64 = 8.;
/// The preview column: its picture, how far it sits from the top, and its info's font size.
const PREVIEW: f64 = 160.;
const PREVIEW_TOP: f64 = 24.;
const PREVIEW_INFO_SIZE: f64 = 11.;

define_class!(
    // A browser row drawn the way Finder draws it: a rounded highlight inset from the column's
    // edges, blue in the column being worked in and grey in the ones before it, and the icon
    // and the name centred in it.
    #[unsafe(super(NSBrowserCell, NSCell, objc2::runtime::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryColumnCell"]
    struct RoundCell;

    impl RoundCell {
        #[unsafe(method(highlightColorInView:))]
        fn highlight_color(&self, _view: &NSView) -> *mut NSColor {
            Retained::autorelease_return(NSColor::clearColor())
        }

        // Everything is drawn in drawWithFrame:inView:; an item-based browser also asks for
        // the interior on its own, which would draw AppKit's row over it.
        #[unsafe(method(drawInteriorWithFrame:inView:))]
        fn draw_interior(&self, _frame: NSRect, _view: &NSView) {}

        #[unsafe(method(drawWithFrame:inView:))]
        fn draw(&self, frame: NSRect, view: &NSView) {
            // The row across the whole column: an item-based browser hands the cell its
            // interior too, indented, and the row must come out the same either way.
            let bounds = view.bounds();
            let frame = NSRect::new(
                NSPoint::new(bounds.origin.x, frame.origin.y),
                NSSize::new(bounds.size.width, frame.size.height),
            );
            let highlighted: bool = unsafe { msg_send![self, isHighlighted] };
            let leaf: bool = unsafe { msg_send![self, isLeaf] };
            // Blue only where the keyboard is: the column whose matrix has focus in the key
            // window. The path to it stays picked in grey.
            let focused = view.window().is_some_and(|window| {
                window.isKeyWindow()
                    && window
                        .firstResponder()
                        .is_some_and(|responder| std::ptr::eq(&*responder as *const _ as *const NSView, view))
            });
            let emphasized = highlighted && focused;
            let inner = NSRect::new(
                NSPoint::new(frame.origin.x + ROW_INSET, frame.origin.y + ROW_GAP),
                NSSize::new(
                    frame.size.width - ROW_INSET * 2.,
                    frame.size.height - ROW_GAP * 2.,
                ),
            );
            if highlighted {
                if emphasized {
                    NSColor::selectedContentBackgroundColor().setFill();
                } else {
                    NSColor::unemphasizedSelectedContentBackgroundColor().setFill();
                }
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    inner, ROW_RADIUS, ROW_RADIUS,
                )
                .fill();
            }
            let middle = frame.origin.y + frame.size.height / 2.;
            let image: Option<Retained<NSImage>> = unsafe { msg_send![self, image] };
            let icon_x = inner.origin.x + ICON_INSET;
            if let Some(image) = image {
                let rect = NSRect::new(
                    NSPoint::new(icon_x, (middle - ICON / 2.).round()),
                    NSSize::new(ICON, ICON),
                );
                unsafe {
                    image.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                        rect,
                        NSRect::ZERO,
                        NSCompositingOperation::SourceOver,
                        1.,
                        true,
                        None,
                    );
                }
            }
            let ink = if emphasized {
                NSColor::alternateSelectedControlTextColor()
            } else {
                NSColor::labelColor()
            };
            let mut text_end = inner.origin.x + inner.size.width - ICON_INSET;
            // A folder's chevron is AppKit's own: the browser draws it at the row's end.
            if !leaf {
                text_end -= CHEVRON_ROOM;
            }
            let name: Retained<NSString> = unsafe { msg_send![self, stringValue] };
            let font = NSFont::systemFontOfSize(FONT_SIZE);
            let line = (font.ascender() - font.descender()).ceil();
            let style = NSMutableParagraphStyle::new();
            style.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            let text_x = icon_x + ICON + ICON_GAP;
            let rect = NSRect::new(
                NSPoint::new(text_x, (middle - line / 2.).round()),
                NSSize::new((text_end - text_x).max(0.), line),
            );
            let attributes = unsafe {
                NSDictionary::<NSString, AnyObject>::from_slices(
                    &[
                        NSFontAttributeName,
                        NSForegroundColorAttributeName,
                        NSParagraphStyleAttributeName,
                    ],
                    &[&*font as &AnyObject, &*ink as &AnyObject, &*style as &AnyObject],
                )
            };
            unsafe { name.drawInRect_withAttributes(rect, Some(&attributes)) };
        }
    }
);

/// What the user did in the column view.
pub enum ColumnEvent {
    /// Picked these rows of the first column, the box's items.
    Pick(Vec<usize>),
    /// Double-clicked this file or folder.
    Open(PathBuf),
}

pub(crate) struct SourceIvars {
    /// The box's items, the first column.
    items: RefCell<Vec<PathBuf>>,
    /// The entries of each folder opened.
    folders: RefCell<HashMap<PathBuf, Vec<PathBuf>>>,
    /// The browser's items: the root, and each path as one string the browser keeps seeing.
    root: Retained<NSString>,
    names: RefCell<HashMap<PathBuf, Retained<NSString>>>,
}

define_class!(
    // The browser's delegate: hands it paths as items.
    #[unsafe(super(objc2::runtime::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryColumnSource"]
    #[ivars = SourceIvars]
    pub(crate) struct Source;

    unsafe impl NSObjectProtocol for Source {}

    impl Source {
        // The item-based delegate, which is the one AppKit gives a preview column to.
        #[unsafe(method(rootItemForBrowser:))]
        fn root_item(&self, _browser: &NSBrowser) -> *mut AnyObject {
            Retained::autorelease_return(self.ivars().root.clone()).cast()
        }

        #[unsafe(method(browser:numberOfChildrenOfItem:))]
        fn children(&self, _browser: &NSBrowser, item: Option<&AnyObject>) -> isize {
            self.children_of(item).len() as isize
        }

        #[unsafe(method(browser:child:ofItem:))]
        fn child(&self, _browser: &NSBrowser, index: isize, item: Option<&AnyObject>) -> *mut AnyObject {
            let Some(path) = self.children_of(item).get(index as usize).cloned() else {
                return std::ptr::null_mut();
            };
            let name = self
                .ivars()
                .names
                .borrow_mut()
                .entry(path.clone())
                .or_insert_with(|| NSString::from_str(&path.to_string_lossy()))
                .clone();
            Retained::autorelease_return(name).cast()
        }

        #[unsafe(method(browser:isLeafItem:))]
        fn is_leaf(&self, _browser: &NSBrowser, item: Option<&AnyObject>) -> bool {
            item_path(item).is_none_or(|path| !path.is_dir())
        }

        #[unsafe(method(browser:objectValueForItem:))]
        fn object_value(&self, _browser: &NSBrowser, item: Option<&AnyObject>) -> *mut AnyObject {
            let name = item_path(item)
                .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
                .unwrap_or_default();
            Retained::autorelease_return(NSString::from_str(&name)).cast()
        }

        #[unsafe(method(browser:previewViewControllerForLeafItem:))]
        fn preview(&self, _browser: &NSBrowser, item: Option<&AnyObject>) -> *mut AnyObject {
            let Some(path) = item_path(item) else {
                return std::ptr::null_mut();
            };
            let Some(mtm) = MainThreadMarker::new() else {
                return std::ptr::null_mut();
            };
            Retained::autorelease_return(preview_controller(mtm, &path)).cast()
        }

        #[unsafe(method(browser:willDisplayCell:atRow:column:))]
        fn will_display(&self, browser: &NSBrowser, cell: &AnyObject, row: isize, column: isize) {
            let matrix: *mut AnyObject = unsafe { msg_send![browser, matrixInColumn: column] };
            if let Some(matrix) = unsafe { matrix.as_ref() } {
                let size: NSSize = unsafe { msg_send![matrix, cellSize] };
                if size.height != ROW_HEIGHT {
                    unsafe {
                        let _: () = msg_send![
                            matrix,
                            setCellSize: NSSize::new(size.width, ROW_HEIGHT)
                        ];
                        let _: () = msg_send![matrix, sizeToCells];
                    }
                }
                let scroll: *mut NSScrollView = unsafe { msg_send![matrix, enclosingScrollView] };
                if let Some(scroll) = unsafe { scroll.as_ref() } {
                    pad(scroll);
                }
            }
            let Some(path) = self.entries(browser, column).get(row as usize).cloned() else {
                return;
            };
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let icon = NSWorkspace::sharedWorkspace()
                .iconForFile(&NSString::from_str(&path.to_string_lossy()));
            icon.setSize(NSSize::new(16., 16.));
            unsafe {
                let _: () = msg_send![cell, setStringValue: &*NSString::from_str(&name)];
                let _: () = msg_send![cell, setLeaf: !path.is_dir()];
                let _: () = msg_send![cell, setFont: &*NSFont::systemFontOfSize(FONT_SIZE)];
                let _: () = msg_send![cell, setImage: &*icon];
            }
        }
    }
);

/// The path a browser item stands for; the root is none.
fn item_path(item: Option<&AnyObject>) -> Option<PathBuf> {
    let text = item?.downcast_ref::<NSString>()?.to_string();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

/// Finder's preview column for a file: its picture as Finder's icon view shows it, its name,
/// and its kind and size, centred at the top of the column.
fn preview_controller(mtm: MainThreadMarker, path: &Path) -> Retained<NSViewController> {
    let picture = crate::finder::thumbnail_image(path, PREVIEW)
        .unwrap_or_else(|| crate::finder::icon(path));
    let image = NSImageView::imageViewWithImage(&picture, mtm);
    image.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    image.setTranslatesAutoresizingMaskIntoConstraints(false);
    image.widthAnchor().constraintEqualToConstant(PREVIEW).setActive(true);
    image.heightAnchor().constraintEqualToConstant(PREVIEW).setActive(true);
    let name = NSTextField::wrappingLabelWithString(
        &NSString::from_str(
            &path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
        mtm,
    );
    name.setFont(Some(&NSFont::boldSystemFontOfSize(FONT_SIZE)));
    name.setAlignment(objc2_app_kit::NSTextAlignment::Center);
    name.setMaximumNumberOfLines(2);
    if let Some(cell) = name.cell() {
        cell.setTruncatesLastVisibleLine(true);
    }
    let size = std::fs::metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file())
        .map(|metadata| crate::finder::size(metadata.len()));
    let info = [crate::finder::kind(path), size]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" – ");
    let details = NSTextField::labelWithString(&NSString::from_str(&info), mtm);
    details.setFont(Some(&NSFont::systemFontOfSize(PREVIEW_INFO_SIZE)));
    details.setTextColor(Some(&NSColor::secondaryLabelColor()));
    details.setAlignment(objc2_app_kit::NSTextAlignment::Center);
    let stack = NSStackView::stackViewWithViews(
        &NSArray::from_retained_slice(&[
            Retained::into_super(Retained::into_super(image)),
            Retained::into_super(Retained::into_super(name.clone())),
            Retained::into_super(Retained::into_super(details)),
        ]),
        mtm,
    );
    stack.setOrientation(objc2_app_kit::NSUserInterfaceLayoutOrientation::Vertical);
    stack.setAlignment(objc2_app_kit::NSLayoutAttribute::CenterX);
    stack.setSpacing(6.);
    stack.setCustomSpacing_afterView(12., &stack.arrangedSubviews().objectAtIndex(0));
    stack.setTranslatesAutoresizingMaskIntoConstraints(false);
    let view = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::ZERO, NSSize::new(COLUMN_WIDTH, PREVIEW * 2.)),
    );
    view.addSubview(&stack);
    stack
        .topAnchor()
        .constraintEqualToAnchor_constant(&view.topAnchor(), PREVIEW_TOP)
        .setActive(true);
    stack
        .centerXAnchor()
        .constraintEqualToAnchor(&view.centerXAnchor())
        .setActive(true);
    name.widthAnchor()
        .constraintLessThanOrEqualToAnchor_constant(&view.widthAnchor(), -2. * ROW_INSET)
        .setActive(true);
    let controller = NSViewController::new(mtm);
    controller.setView(&view);
    controller
}

impl Source {
    /// The paths under browser item `item`: the box's items under the root, a folder's
    /// entries, read the first time it is opened, under the folder.
    fn children_of(&self, item: Option<&AnyObject>) -> Vec<PathBuf> {
        match item_path(item) {
            None => self.ivars().items.borrow().clone(),
            Some(folder) => self
                .ivars()
                .folders
                .borrow_mut()
                .entry(folder.clone())
                .or_insert_with(|| list(&folder))
                .clone(),
        }
    }

    /// What column `column` lists: the box's items first, then the entries of the folder
    /// picked in each column before, read the first time it is opened.
    fn entries(&self, browser: &NSBrowser, column: isize) -> Vec<PathBuf> {
        let mut entries = self.ivars().items.borrow().clone();
        for before in 0..column.max(0) {
            let row: isize = unsafe { msg_send![browser, selectedRowInColumn: before] };
            let Some(folder) = (row >= 0)
                .then(|| entries.get(row as usize).cloned())
                .flatten()
            else {
                return Vec::new();
            };
            entries = self
                .ivars()
                .folders
                .borrow_mut()
                .entry(folder.clone())
                .or_insert_with(|| list(&folder))
                .clone();
        }
        entries
    }
}

/// A folder's entries sorted by name the way Finder sorts them, hidden files left out.
fn list(folder: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            !path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        })
        .collect();
    entries.sort_by_key(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
    });
    entries.truncate(LIMIT);
    entries
}

/// Gives a column Finder's room above its first row and below its last, on top of the toolbar
/// AppKit already keeps it clear of; once, when it first shows.
fn pad(scroll: &NSScrollView) {
    if !scroll.automaticallyAdjustsContentInsets() {
        return;
    }
    let insets = scroll.contentInsets();
    scroll.setAutomaticallyAdjustsContentInsets(false);
    scroll.setContentInsets(objc2_foundation::NSEdgeInsets {
        top: insets.top + COLUMN_PAD,
        bottom: insets.bottom + COLUMN_PAD,
        ..insets
    });
    let clip = scroll.contentView();
    clip.scrollToPoint(NSPoint::new(0., -(insets.top + COLUMN_PAD)));
    scroll.reflectScrolledClipView(&clip);
}

/// Keeps the toolbar's bar showing over the columns, as Finder's does, even before anything
/// has scrolled under it: macOS 27 otherwise leaves the bar clear while the scroll view under
/// the toolbar is at its top.
fn always_show_bar(browser: &NSBrowser) {
    for child in browser.subviews() {
        let scroll: &AnyObject = &child;
        let responds: bool =
            unsafe { msg_send![scroll, respondsToSelector: sel!(setAlwaysShownPocketEdges:)] };
        if !responds {
            continue;
        }
        let allowed: usize = unsafe { msg_send![scroll, allowedPocketEdges] };
        unsafe {
            let _: () = msg_send![scroll, setAlwaysShownPocketEdges: allowed];
        }
    }
}

/// The column view.
pub struct ColumnBrowser {
    native: NativeView,
    browser: Retained<NSBrowser>,
    source: Retained<Source>,
    shown: RefCell<Vec<PathBuf>>,
}

impl ColumnBrowser {
    /// # Safety
    /// `ns_view` must point to a live `NSView`, and this must run on the main thread.
    pub unsafe fn new(
        ns_view: NonNull<c_void>,
        on_event: impl Fn(ColumnEvent) + 'static,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let parent: &NSView = unsafe { ns_view.cast().as_ref() };
        let frame = NSRect::new(NSPoint::ZERO, NSSize::new(400., 300.));
        let browser = NSBrowser::initWithFrame(NSBrowser::alloc(mtm), frame);
        let source = Source::alloc(mtm).set_ivars(SourceIvars {
            items: RefCell::new(Vec::new()),
            folders: RefCell::new(HashMap::new()),
            root: NSString::new(),
            names: RefCell::new(HashMap::new()),
        });
        let source: Retained<Source> = unsafe { msg_send![super(source), init] };
        let target = Target::new(mtm, {
            let browser = browser.clone();
            let source = source.clone();
            move |_| {
                let double = objc2_app_kit::NSApplication::sharedApplication(mtm)
                    .currentEvent()
                    .is_some_and(|event| event.clickCount() >= 2);
                if double {
                    let row: isize = unsafe { msg_send![&*browser, clickedRow] };
                    let column: isize = unsafe { msg_send![&*browser, clickedColumn] };
                    if let Some(path) = source.entries(&browser, column).get(row as usize) {
                        on_event(ColumnEvent::Open(path.clone()));
                    }
                    return;
                }
                let rows: Retained<NSIndexSet> =
                    unsafe { msg_send![&*browser, selectedRowIndexesInColumn: 0isize] };
                let mut picked = Vec::new();
                let mut index = rows.firstIndex();
                while index != objc2_foundation::NSNotFound as usize {
                    picked.push(index);
                    index = rows.indexGreaterThanIndex(index);
                }
                on_event(ColumnEvent::Pick(picked));
            }
        });
        unsafe {
            let _: () = msg_send![&*browser, setDelegate: &*source];
            let _: () = msg_send![&*browser, setTarget: &*target];
            let _: () = msg_send![&*browser, setAction: sel!(fire:)];
        }
        browser.setTitled(false);
        unsafe {
            let _: () =
                msg_send![&*browser, setCellClass: <RoundCell as objc2::ClassType>::class()];
        }
        browser.setAllowsMultipleSelection(true);
        browser.setHasHorizontalScroller(true);
        browser.setAutohidesScroller(true);
        browser.setSeparatesColumns(true);
        browser.setDefaultColumnWidth(COLUMN_WIDTH);
        browser
            .setColumnResizingType(objc2_app_kit::NSBrowserColumnResizingType::UserColumnResizing);
        browser.setReusesColumns(false);
        always_show_bar(&browser);
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(browser.clone()));
        Some(Self {
            native: NativeView::host(parent, view, Some(target)),
            browser,
            source,
            shown: RefCell::new(Vec::new()),
        })
    }

    pub fn native(&self) -> &NativeView {
        &self.native
    }

    /// Lists `paths`, the box's items, in the first column.
    pub fn set_items(&self, paths: &[PathBuf]) {
        if *self.shown.borrow() == paths {
            return;
        }
        *self.shown.borrow_mut() = paths.to_vec();
        *self.source.ivars().items.borrow_mut() = paths.to_vec();
        self.source.ivars().folders.borrow_mut().clear();
        self.source.ivars().names.borrow_mut().clear();
        self.browser.loadColumnZero();
    }
}
