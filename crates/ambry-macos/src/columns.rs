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
    NSFontAttributeName, NSForegroundColorAttributeName, NSImage, NSImageSymbolConfiguration,
    NSImageSymbolScale, NSLineBreakMode, NSMutableParagraphStyle, NSParagraphStyleAttributeName,
    NSStringDrawing, NSView, NSWorkspace,
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
/// Finder's chevron after a folder: its size, and how far it keeps from the highlight's end.
const CHEVRON: f64 = 10.;
const CHEVRON_INSET: f64 = 8.;
/// The pocket edges AppKit allows a plain scroll view by default.
const POCKET_EDGES: usize = 3;

define_class!(
    // A browser row drawn the way Finder draws it: a rounded highlight inset from the column's
    // edges, blue in the column being worked in and grey in the ones before it, the icon and
    // the name centred in it, and a chevron after a folder.
    #[unsafe(super(NSBrowserCell, NSCell, objc2::runtime::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryColumnCell"]
    struct RoundCell;

    impl RoundCell {
        #[unsafe(method(highlightColorInView:))]
        fn highlight_color(&self, _view: &NSView) -> *mut NSColor {
            Retained::autorelease_return(NSColor::clearColor())
        }

        #[unsafe(method(drawWithFrame:inView:))]
        fn draw(&self, frame: NSRect, view: &NSView) {
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
            if !leaf {
                if let Some(chevron) = chevron(if emphasized {
                    NSColor::alternateSelectedControlTextColor()
                } else {
                    NSColor::secondaryLabelColor()
                }) {
                    let size = chevron.size();
                    let x = inner.origin.x + inner.size.width - CHEVRON_INSET - size.width;
                    let rect = NSRect::new(
                        NSPoint::new(x.round(), (middle - size.height / 2.).round()),
                        size,
                    );
                    unsafe {
                        chevron.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                            rect,
                            NSRect::ZERO,
                            NSCompositingOperation::SourceOver,
                            1.,
                            true,
                            None,
                        );
                    }
                    text_end = x - ICON_GAP;
                }
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

/// Finder's chevron after a folder, in `color`.
fn chevron(color: Retained<NSColor>) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str("chevron.right"),
        None,
    )?;
    let size = NSImageSymbolConfiguration::configurationWithPointSize_weight_scale(
        CHEVRON,
        unsafe { objc2_app_kit::NSFontWeightSemibold },
        NSImageSymbolScale::Small,
    );
    let tint = NSImageSymbolConfiguration::configurationWithPaletteColors(
        &NSArray::from_retained_slice(&[color]),
    );
    image.imageWithSymbolConfiguration(&size.configurationByApplyingConfiguration(&tint))
}

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
        #[unsafe(method(browser:numberOfRowsInColumn:))]
        fn rows(&self, browser: &NSBrowser, column: isize) -> isize {
            self.entries(browser, column).len() as isize
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
                let scroll: *mut AnyObject = unsafe { msg_send![matrix, enclosingScrollView] };
                if let Some(scroll) = unsafe { scroll.as_ref() } {
                    allow_pocket(scroll);
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

/// Lets a column run under the toolbar's scroll edge effect, as Finder's columns do. AppKit
/// gives every scroll view the toolbar's pocket on macOS 26 except a browser's columns.
fn allow_pocket(scroll: &AnyObject) {
    let responds: bool =
        unsafe { msg_send![scroll, respondsToSelector: sel!(setAllowedPocketEdges:)] };
    if !responds {
        return;
    }
    let allowed: usize = unsafe { msg_send![scroll, allowedPocketEdges] };
    if allowed != POCKET_EDGES {
        unsafe {
            let _: () = msg_send![scroll, setAllowedPocketEdges: POCKET_EDGES];
        }
    }
}

impl Source {
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
        self.browser.loadColumnZero();
    }
}
