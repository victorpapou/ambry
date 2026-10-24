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
    NSBezierPath, NSBrowser, NSBrowserCell, NSCell, NSColor, NSFont, NSView, NSWorkspace,
};
use objc2_foundation::{NSIndexSet, NSPoint, NSRect, NSSize, NSString};

use crate::native::{NativeView, Target};

/// The width each column opens at.
const COLUMN_WIDTH: f64 = 240.;
/// The most entries one folder's column lists.
const LIMIT: usize = 500;

/// Height of a row, how far its highlight keeps from the column's sides, and its corners:
/// Finder's column view on macOS 26.
const ROW_HEIGHT: f64 = 24.;
const ROW_INSET: f64 = 6.;
const ROW_RADIUS: f64 = 6.;
const FONT_SIZE: f64 = 13.;

define_class!(
    // A browser row drawn the way Finder draws it: a rounded highlight inset from the column's
    // edges instead of a bar across it.
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
            let inner = NSRect::new(
                NSPoint::new(frame.origin.x + ROW_INSET, frame.origin.y + 1.),
                NSSize::new(frame.size.width - ROW_INSET * 2., frame.size.height - 2.),
            );
            if highlighted {
                NSColor::selectedContentBackgroundColor().setFill();
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    inner, ROW_RADIUS, ROW_RADIUS,
                )
                .fill();
            }
            let text = NSRect::new(
                NSPoint::new(inner.origin.x + 4., frame.origin.y),
                NSSize::new(inner.size.width - 4., frame.size.height),
            );
            unsafe {
                let _: () = msg_send![super(self), drawInteriorWithFrame: text, inView: view];
            }
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
            let _: () = msg_send![&*browser, setCellClass: RoundCell::class()];
            let _: () = msg_send![&*browser, setRowHeight: ROW_HEIGHT];
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
