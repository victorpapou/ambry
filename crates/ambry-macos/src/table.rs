//! The list view: an NSTableView with Finder's columns.

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSColor, NSControlTextEditingDelegate, NSImage, NSImageView, NSLineBreakMode, NSScrollView,
    NSStackView, NSTableColumn, NSTableView, NSTableViewColumnAutoresizingStyle,
    NSTableViewDataSource, NSTableViewDelegate, NSTableViewStyle, NSTextField, NSView,
};
use objc2_foundation::{
    NSArray, NSEdgeInsets, NSInteger, NSNotification, NSRect, NSSize, NSString,
};

use crate::finder;
use crate::native::{NativeView, Target};

const ROW_HEIGHT: f64 = 24.;
const ICON_SIZE: f64 = 16.;
const COLUMNS: [(&str, &str, f64); 4] = [
    ("name", "Name", 280.),
    ("modified", "Date Modified", 180.),
    ("size", "Size", 80.),
    ("kind", "Kind", 160.),
];

/// One file in the table.
pub struct FileRow {
    pub path: PathBuf,
    pub name: String,
}

struct Row {
    icon: Retained<NSImage>,
    name: String,
    modified: String,
    size: String,
    kind: String,
    exists: bool,
}

impl Row {
    fn new(file: &FileRow) -> Self {
        let metadata = std::fs::metadata(&file.path).ok();
        let modified = metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok())
            .map(finder::date);
        let size = metadata
            .as_ref()
            .filter(|metadata| metadata.is_file())
            .map(|metadata| finder::size(metadata.len()));
        let icon = finder::icon(&file.path);
        icon.setSize(NSSize::new(ICON_SIZE, ICON_SIZE));
        Self {
            icon,
            name: file.name.clone(),
            modified: modified.unwrap_or_else(|| "--".into()),
            size: size.unwrap_or_else(|| "--".into()),
            kind: finder::kind(&file.path).unwrap_or_default(),
            exists: metadata.is_some(),
        }
    }
}

struct SourceIvars {
    rows: RefCell<Vec<Row>>,
    on_select: Box<dyn Fn(Option<usize>)>,
}

define_class!(
    // Feeds the rows to the table and reports selection changes.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryFileTableSource"]
    #[ivars = SourceIvars]
    struct Source;

    unsafe impl NSObjectProtocol for Source {}

    unsafe impl NSTableViewDataSource for Source {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _table: &NSTableView) -> NSInteger {
            self.ivars().rows.borrow().len() as NSInteger
        }
    }

    unsafe impl NSControlTextEditingDelegate for Source {}

    unsafe impl NSTableViewDelegate for Source {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for(
            &self,
            _table: &NSTableView,
            column: Option<&NSTableColumn>,
            row: NSInteger,
        ) -> Option<Retained<NSView>> {
            self.view_for_row(column, row)
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_changed(&self, notification: &NSNotification) {
            let Some(table) = notification
                .object()
                .and_then(|object| object.downcast::<NSTableView>().ok())
            else {
                return;
            };
            (self.ivars().on_select)(usize::try_from(table.selectedRow()).ok());
        }
    }
);

impl Source {
    fn view_for_row(
        &self,
        column: Option<&NSTableColumn>,
        row: NSInteger,
    ) -> Option<Retained<NSView>> {
        let rows = self.ivars().rows.borrow();
        let row = rows.get(usize::try_from(row).ok()?)?;
        cell(self.mtm(), &column?.identifier().to_string(), row)
    }
}

/// Finder-style list of files: icon and name, date modified, size and kind.
pub struct FileTable {
    native: NativeView,
    scroll: Retained<NSScrollView>,
    table: Retained<NSTableView>,
    source: Retained<Source>,
}

impl FileTable {
    /// Adds an empty table to `ns_view`. `on_open` gets the row that was double-clicked,
    /// `on_select` the selected row.
    ///
    /// # Safety
    /// `ns_view` must point to a live `NSView`, and this must run on the main thread.
    pub unsafe fn new(
        ns_view: NonNull<c_void>,
        on_open: impl Fn(usize) + 'static,
        on_select: impl Fn(Option<usize>) + 'static,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let parent: &NSView = unsafe { ns_view.cast().as_ref() };

        let table = NSTableView::initWithFrame(NSTableView::alloc(mtm), NSRect::ZERO);
        table.setStyle(NSTableViewStyle::Inset);
        table.setUsesAlternatingRowBackgroundColors(true);
        table.setRowHeight(ROW_HEIGHT);
        table.setAllowsMultipleSelection(false);
        table.setColumnAutoresizingStyle(
            NSTableViewColumnAutoresizingStyle::FirstColumnOnlyAutoresizingStyle,
        );
        for (id, title, width) in COLUMNS {
            let column = NSTableColumn::initWithIdentifier(
                NSTableColumn::alloc(mtm),
                &NSString::from_str(id),
            );
            column.setTitle(&NSString::from_str(title));
            column.setWidth(width);
            column.setMinWidth(60.);
            table.addTableColumn(&column);
        }

        let source = Source::alloc(mtm).set_ivars(SourceIvars {
            rows: RefCell::new(Vec::new()),
            on_select: Box::new(on_select),
        });
        let source: Retained<Source> = unsafe { msg_send![super(source), init] };
        let target = Target::new(mtm, move |sender| {
            let Some(table) = sender.downcast_ref::<NSTableView>() else {
                return;
            };
            if let Ok(row) = usize::try_from(table.clickedRow()) {
                on_open(row);
            }
        });
        unsafe {
            table.setDataSource(Some(ProtocolObject::from_ref(&*source)));
            table.setDelegate(Some(ProtocolObject::from_ref(&*source)));
            table.setTarget(Some(&target));
            table.setDoubleAction(Some(sel!(fire:)));
        }

        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), NSRect::ZERO);
        scroll.setDocumentView(Some(&table));
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        scroll.setAutomaticallyAdjustsContentInsets(false);
        let view: Retained<NSView> = Retained::into_super(scroll.clone());
        Some(Self {
            native: NativeView::host(parent, view, target),
            scroll,
            table,
            source,
        })
    }

    pub fn native(&self) -> &NativeView {
        &self.native
    }

    /// Leaves room under the last row for controls floating over the table.
    pub fn set_bottom_inset(&self, inset: f64) {
        self.scroll.setContentInsets(NSEdgeInsets {
            top: 0.,
            left: 0.,
            bottom: inset,
            right: 0.,
        });
    }

    /// Replaces the rows and selects `selected`.
    pub fn set_rows(&self, files: &[FileRow], selected: Option<usize>) {
        *self.source.ivars().rows.borrow_mut() = files.iter().map(Row::new).collect();
        self.table.reloadData();
        self.select(selected);
    }

    /// Selects a row without reporting it back.
    pub fn select(&self, row: Option<usize>) {
        let current = usize::try_from(self.table.selectedRow()).ok();
        if current == row {
            return;
        }
        match row {
            Some(row) => {
                let indexes = objc2_foundation::NSIndexSet::indexSetWithIndex(row);
                self.table
                    .selectRowIndexes_byExtendingSelection(&indexes, false);
            }
            None => unsafe { self.table.deselectAll(None) },
        }
    }
}

/// The view for one cell: the icon and name, or a quieter text column.
fn cell(mtm: MainThreadMarker, column: &str, row: &Row) -> Option<Retained<NSView>> {
    let text = match column {
        "name" => &row.name,
        "modified" => &row.modified,
        "size" => &row.size,
        _ => &row.kind,
    };
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
    let mut views: Vec<Retained<NSView>> = Vec::new();
    if column == "name" {
        let icon = NSImageView::imageViewWithImage(&row.icon, mtm);
        views.push(Retained::into_super(Retained::into_super(icon)));
    } else {
        label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    }
    if !row.exists {
        label.setTextColor(Some(&NSColor::tertiaryLabelColor()));
    }
    views.push(Retained::into_super(Retained::into_super(label)));
    let stack = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(&views), mtm);
    stack.setSpacing(6.);
    Some(Retained::into_super(stack))
}
