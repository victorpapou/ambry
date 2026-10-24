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
use objc2_app_kit::{NSBrowser, NSView, NSWorkspace};
use objc2_foundation::{NSIndexSet, NSPoint, NSRect, NSSize, NSString};

use crate::native::{NativeView, Target};

/// The width each column opens at.
const COLUMN_WIDTH: f64 = 240.;
/// The most entries one folder's column lists.
const LIMIT: usize = 500;

/// What the user did in the column view.
pub enum ColumnEvent {
    /// Picked these rows of the first column, the box's items.
    Pick(Vec<usize>),
    /// Double-clicked this file or folder.
    Open(PathBuf),
}

pub(crate) struct SourceIvars {
    root: Retained<NSString>,
    /// The box's items, the first column.
    items: RefCell<Vec<Retained<NSString>>>,
    /// The entries of each folder opened, kept so AppKit's pointers to them stay alive.
    folders: RefCell<HashMap<String, Vec<Retained<NSString>>>>,
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
        #[unsafe(method(rootItemForBrowser:))]
        fn root_item(&self, _browser: &NSBrowser) -> *mut AnyObject {
            Retained::as_ptr(&self.ivars().root) as *mut AnyObject
        }

        #[unsafe(method(browser:numberOfChildrenOfItem:))]
        fn count(&self, _browser: &NSBrowser, item: Option<&AnyObject>) -> isize {
            self.with_children(item, |children| children.len() as isize)
        }

        #[unsafe(method(browser:child:ofItem:))]
        fn child(&self, _browser: &NSBrowser, index: isize, item: Option<&AnyObject>) -> *mut AnyObject {
            self.with_children(item, |children| {
                children
                    .get(index as usize)
                    .map_or(std::ptr::null_mut(), |child| {
                        Retained::as_ptr(child) as *mut AnyObject
                    })
            })
        }

        #[unsafe(method(browser:isLeafItem:))]
        fn is_leaf(&self, _browser: &NSBrowser, item: Option<&AnyObject>) -> bool {
            item.and_then(path_of).is_none_or(|path| !path.is_dir())
        }

        #[unsafe(method(browser:objectValueForItem:))]
        fn value(&self, _browser: &NSBrowser, item: Option<&AnyObject>) -> *mut AnyObject {
            let name = item
                .and_then(path_of)
                .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
                .unwrap_or_default();
            Retained::autorelease_return(NSString::from_str(&name)) as *mut AnyObject
        }

        #[unsafe(method(browser:willDisplayCell:atRow:column:))]
        fn will_display(&self, browser: &NSBrowser, cell: &AnyObject, row: isize, column: isize) {
            let item: *mut AnyObject = unsafe { msg_send![browser, itemAtRow: row, inColumn: column] };
            let Some(path) = (unsafe { item.as_ref() }).and_then(path_of) else {
                return;
            };
            let icon = NSWorkspace::sharedWorkspace()
                .iconForFile(&NSString::from_str(&path.to_string_lossy()));
            icon.setSize(NSSize::new(16., 16.));
            let _: () = unsafe { msg_send![cell, setImage: &*icon] };
        }
    }
);

impl Source {
    /// Runs `f` on the children of `item`: the box's items for the root, else the folder's
    /// entries, read the first time it is opened.
    fn with_children<R>(
        &self,
        item: Option<&AnyObject>,
        f: impl FnOnce(&[Retained<NSString>]) -> R,
    ) -> R {
        let root: &AnyObject = &self.ivars().root;
        if item.is_none_or(|item| std::ptr::eq(item, root)) {
            return f(&self.ivars().items.borrow());
        }
        let Some(path) = item.and_then(path_of) else {
            return f(&[]);
        };
        let key = path.to_string_lossy().into_owned();
        let mut folders = self.ivars().folders.borrow_mut();
        let children = folders.entry(key).or_insert_with(|| list(&path));
        f(children)
    }
}

fn path_of(item: &AnyObject) -> Option<PathBuf> {
    let text: *mut NSString = item as *const AnyObject as *mut NSString;
    let text = unsafe { text.as_ref() }?.to_string();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

/// A folder's entries sorted by name the way Finder sorts them, hidden files left out.
fn list(folder: &Path) -> Vec<Retained<NSString>> {
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
        .iter()
        .map(|path| NSString::from_str(&path.to_string_lossy()))
        .collect()
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
            root: NSString::from_str(""),
            items: RefCell::new(Vec::new()),
            folders: RefCell::new(HashMap::new()),
        });
        let source: Retained<Source> = unsafe { msg_send![super(source), init] };
        let target = Target::new(mtm, {
            let browser = browser.clone();
            move |_| {
                let double = objc2_app_kit::NSApplication::sharedApplication(mtm)
                    .currentEvent()
                    .is_some_and(|event| event.clickCount() >= 2);
                if double {
                    let row: isize = unsafe { msg_send![&*browser, clickedRow] };
                    let column: isize = unsafe { msg_send![&*browser, clickedColumn] };
                    let item: *mut AnyObject =
                        unsafe { msg_send![&*browser, itemAtRow: row, inColumn: column] };
                    if let Some(path) = (unsafe { item.as_ref() }).and_then(path_of) {
                        on_event(ColumnEvent::Open(path));
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
        *self.source.ivars().items.borrow_mut() = paths
            .iter()
            .map(|path| NSString::from_str(&path.to_string_lossy()))
            .collect();
        self.source.ivars().folders.borrow_mut().clear();
        self.browser.loadColumnZero();
    }
}
