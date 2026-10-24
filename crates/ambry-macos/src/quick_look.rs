//! Quick Look, as Finder opens it with the space bar: the system's preview panel over the
//! selected file or folder, with the box's other items a step away.
//!
//! The panel asks the responder chain for a controller, so the window GPUI made learns to
//! answer it: the methods are added to its class the first time a preview opens.

use std::cell::{Cell, RefCell};
use std::ffi::CStr;
use std::path::PathBuf;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSEvent, NSEventType, NSWindowDelegate};
use objc2_foundation::{NSInteger, NSString, NSURL};
use objc2_quick_look_ui::{QLPreviewPanel, QLPreviewPanelDataSource, QLPreviewPanelDelegate};

/// Key codes of the keys the panel hands back: space and the arrows.
const SPACE: u16 = 49;
const LEFT: u16 = 123;
const RIGHT: u16 = 124;
const DOWN: u16 = 125;
const UP: u16 = 126;

struct Items {
    urls: RefCell<Vec<Retained<NSURL>>>,
    /// Hears the index of the item the panel moved to with the arrow keys.
    on_move: RefCell<Option<Rc<dyn Fn(usize)>>>,
}

define_class!(
    // Gives the panel its items and takes the keys it doesn't use itself.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryQuickLookItems"]
    #[ivars = Items]
    struct PreviewItems;

    unsafe impl NSObjectProtocol for PreviewItems {}

    unsafe impl QLPreviewPanelDataSource for PreviewItems {
        #[unsafe(method(numberOfPreviewItemsInPreviewPanel:))]
        fn count(&self, _panel: Option<&QLPreviewPanel>) -> NSInteger {
            self.ivars().urls.borrow().len() as NSInteger
        }

        #[unsafe(method_id(previewPanel:previewItemAtIndex:))]
        fn item(
            &self,
            _panel: Option<&QLPreviewPanel>,
            index: NSInteger,
        ) -> Option<Retained<NSURL>> {
            self.ivars().urls.borrow().get(index as usize).cloned()
        }
    }

    unsafe impl NSWindowDelegate for PreviewItems {}

    unsafe impl QLPreviewPanelDelegate for PreviewItems {
        #[unsafe(method(previewPanel:handleEvent:))]
        fn handle_event(&self, panel: Option<&QLPreviewPanel>, event: Option<&NSEvent>) -> Bool {
            let (Some(panel), Some(event)) = (panel, event) else {
                return Bool::NO;
            };
            if event.r#type() != NSEventType::KeyDown {
                return Bool::NO;
            }
            let count = self.ivars().urls.borrow().len() as NSInteger;
            let current = unsafe { panel.currentPreviewItemIndex() };
            let next = match event.keyCode() {
                SPACE => {
                    panel.orderOut(None);
                    return Bool::YES;
                }
                LEFT | UP => current - 1,
                RIGHT | DOWN => current + 1,
                _ => return Bool::NO,
            };
            if (0..count).contains(&next) {
                unsafe { panel.setCurrentPreviewItemIndex(next) };
                let on_move = self.ivars().on_move.borrow().clone();
                if let Some(on_move) = on_move {
                    on_move(next as usize);
                }
            }
            Bool::YES
        }
    }
);

thread_local! {
    static ITEMS: RefCell<Option<Retained<PreviewItems>>> = const { RefCell::new(None) };
    static TAUGHT: Cell<bool> = const { Cell::new(false) };
}

fn items(mtm: MainThreadMarker) -> Retained<PreviewItems> {
    ITEMS.with_borrow_mut(|items| {
        items
            .get_or_insert_with(|| {
                let this = PreviewItems::alloc(mtm).set_ivars(Items {
                    urls: RefCell::new(Vec::new()),
                    on_move: RefCell::new(None),
                });
                unsafe { msg_send![super(this), init] }
            })
            .clone()
    })
}

/// Opens Quick Look on `paths[current]`, or closes it when it is open. `on_move` hears the
/// item the panel moves to with the arrow keys.
pub fn toggle(paths: &[PathBuf], current: usize, on_move: impl Fn(usize) + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if is_open(mtm) {
        unsafe { QLPreviewPanel::sharedPreviewPanel(mtm) }.map(|panel| panel.orderOut(None));
        return;
    }
    let Some(window) = NSApplication::sharedApplication(mtm).keyWindow() else {
        return;
    };
    teach(window.class());
    let items = items(mtm);
    *items.ivars().on_move.borrow_mut() = Some(Rc::new(on_move));
    set_paths(&items, paths);
    let Some(panel) = (unsafe { QLPreviewPanel::sharedPreviewPanel(mtm) }) else {
        return;
    };
    unsafe {
        panel.updateController();
        panel.reloadData();
        panel.setCurrentPreviewItemIndex(current as NSInteger);
    }
    panel.makeKeyAndOrderFront(None);
}

/// Shows `paths[current]` in the panel when it is open, as the selection moves.
pub fn follow(paths: &[PathBuf], current: usize) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if !is_open(mtm) {
        return;
    }
    set_paths(&items(mtm), paths);
    if let Some(panel) = unsafe { QLPreviewPanel::sharedPreviewPanel(mtm) } {
        unsafe {
            panel.reloadData();
            panel.setCurrentPreviewItemIndex(current as NSInteger);
        }
    }
}

fn is_open(mtm: MainThreadMarker) -> bool {
    let exists = unsafe { QLPreviewPanel::sharedPreviewPanelExists(mtm) };
    exists
        && unsafe { QLPreviewPanel::sharedPreviewPanel(mtm) }.is_some_and(|panel| panel.isVisible())
}

fn set_paths(items: &PreviewItems, paths: &[PathBuf]) {
    *items.ivars().urls.borrow_mut() = paths
        .iter()
        .map(|path| NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy())))
        .collect();
}

/// Adds the panel controller's three methods to the window's class.
fn teach(class: &AnyClass) {
    if TAUGHT.replace(true) {
        return;
    }
    extern "C-unwind" fn accepts(_: &AnyObject, _: Sel, _: *mut AnyObject) -> Bool {
        Bool::YES
    }
    extern "C-unwind" fn begin(_: &AnyObject, _: Sel, panel: *mut QLPreviewPanel) {
        let (Some(panel), Some(mtm)) = (unsafe { panel.as_ref() }, MainThreadMarker::new()) else {
            return;
        };
        let items = items(mtm);
        unsafe {
            panel.setDataSource(Some(ProtocolObject::from_ref(&*items)));
            panel.setDelegate(Some(&items));
        }
    }
    extern "C-unwind" fn end(_: &AnyObject, _: Sel, panel: *mut QLPreviewPanel) {
        if let Some(panel) = unsafe { panel.as_ref() } {
            unsafe {
                panel.setDataSource(None);
                panel.setDelegate(None);
            }
        }
    }
    let class = class as *const AnyClass as *mut AnyClass;
    let add = |name: Sel, imp: objc2::runtime::Imp, types: &CStr| unsafe {
        objc2::ffi::class_addMethod(class, name, imp, types.as_ptr());
    };
    unsafe {
        add(
            sel!(acceptsPreviewPanelControl:),
            std::mem::transmute::<extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject) -> Bool, _>(
                accepts,
            ),
            c"B@:@",
        );
        add(
            sel!(beginPreviewPanelControl:),
            std::mem::transmute::<extern "C-unwind" fn(&AnyObject, Sel, *mut QLPreviewPanel), _>(
                begin,
            ),
            c"v@:@",
        );
        add(
            sel!(endPreviewPanelControl:),
            std::mem::transmute::<extern "C-unwind" fn(&AnyObject, Sel, *mut QLPreviewPanel), _>(
                end,
            ),
            c"v@:@",
        );
    }
}
