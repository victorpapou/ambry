//! A lab window with the same switch inside a real NSToolbar, to compare the toolbar look
//! (glass capsule, hover pill, lens on press) with the switch in window content.

use std::cell::OnceCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSSegmentSwitchTracking, NSSegmentedControl, NSToolbar,
    NSToolbarDelegate, NSToolbarItem, NSToolbarItemIdentifier, NSView, NSWindow, NSWindowStyleMask,
    NSWindowToolbarStyle,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize, NSString};

const TOOLS: &[(&str, &str)] = &[
    ("cursorarrow", "Select"),
    ("hand.raised", "Pan"),
    ("plus.magnifyingglass", "Zoom"),
    ("rotate.right", "Rotate"),
    ("scope", "Target"),
];
const MODES: &[(&str, &str)] = &[("number", "Canvas"), ("list.bullet", "List")];
const IDS: [&str; 4] = ["tools", "modes", "tabs", "add"];

#[derive(Default)]
struct LabIvars {
    tools: OnceCell<Retained<NSSegmentedControl>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryLabToolbarDelegate"]
    #[ivars = LabIvars]
    struct LabDelegate;

    unsafe impl NSObjectProtocol for LabDelegate {}

    unsafe impl NSToolbarDelegate for LabDelegate {
        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item(
            &self,
            _toolbar: &NSToolbar,
            id: &NSToolbarItemIdentifier,
            _flag: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            self.make_item(id)
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

    impl LabDelegate {
        #[unsafe(method(dump:))]
        fn dump(&self, _sender: Option<&AnyObject>) {
            if let Some(tools) = self.ivars().tools.get() {
                write_dump(tools);
            }
        }
    }
);

impl LabDelegate {
    fn make_item(&self, id: &NSToolbarItemIdentifier) -> Option<Retained<NSToolbarItem>> {
        let mtm = self.mtm();
        let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), id);
        let view: Retained<NSView> = match id.to_string().as_str() {
            "tools" => {
                let control = segmented(mtm, TOOLS)?;
                let _ = self.ivars().tools.set(control.clone());
                Retained::into_super(Retained::into_super(control))
            }
            "modes" => Retained::into_super(Retained::into_super(segmented(mtm, MODES)?)),
            "tabs" => {
                let images = crate::native::icon_images(MODES)?;
                let tabs = crate::native::icon_tab_view(mtm, MODES.len(), &images, 40.);
                tabs.setDrawsBackground(false);
                Retained::into_super(tabs)
            }
            _ => {
                let image = crate::lab::symbol("plus", "Add")?;
                let button =
                    unsafe { NSButton::buttonWithImage_target_action(&image, None, None, mtm) };
                Retained::into_super(Retained::into_super(button))
            }
        };
        item.setView(Some(&view));
        item.setLabel(id);
        Some(item)
    }
}

pub fn open_toolbar_lab(mtm: MainThreadMarker) {
    let delegate: Retained<LabDelegate> = {
        let this = LabDelegate::alloc(mtm).set_ivars(LabIvars::default());
        unsafe { msg_send![super(this), init] }
    };
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(0., 0.), NSSize::new(720., 260.)),
            NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Resizable
                | NSWindowStyleMask::FullSizeContentView,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("Toolbar lab"));
    window.setToolbarStyle(NSWindowToolbarStyle::Unified);
    let toolbar =
        NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), &NSString::from_str("ambry.lab"));
    toolbar.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    window.setToolbar(Some(&toolbar));
    window.center();
    window.makeKeyAndOrderFront(None);
    // The toolbar builds its views lazily, so dump once it has been on screen for a moment.
    unsafe {
        let _: () = msg_send![
            &*delegate,
            performSelector: sel!(dump:),
            withObject: Option::<&AnyObject>::None,
            afterDelay: 1.0f64
        ];
    }
    std::mem::forget(window);
    std::mem::forget(delegate);
}

fn ids() -> Retained<NSArray<NSToolbarItemIdentifier>> {
    let ids: Vec<Retained<NSString>> = IDS.iter().map(|id| NSString::from_str(id)).collect();
    NSArray::from_retained_slice(&ids)
}

fn segmented(
    mtm: MainThreadMarker,
    icons: &[(&str, &str)],
) -> Option<Retained<NSSegmentedControl>> {
    let images = icons
        .iter()
        .map(|(name, description)| crate::lab::symbol(name, description))
        .collect::<Option<Vec<_>>>()?;
    let control = unsafe {
        NSSegmentedControl::segmentedControlWithImages_trackingMode_target_action(
            &NSArray::from_retained_slice(&images),
            NSSegmentSwitchTracking::SelectOne,
            None,
            None,
            mtm,
        )
    };
    control.setSelectedSegment(0);
    Some(control)
}

/// Writes how AppKit hosts the toolbar's segmented control to `target/lab-toolbar-dump.txt`.
fn write_dump(control: &NSSegmentedControl) {
    let mut out = String::from("== superviews\n");
    let mut view: Option<Retained<NSView>> = Some(control.retain().into_super().into_super());
    let mut top = None;
    while let Some(current) = view {
        out += &format!(
            "{} {:?}\n",
            current.class().name().to_string_lossy(),
            current.frame()
        );
        view = unsafe { current.superview() };
        top = Some(current);
    }
    if let Some(top) = top {
        let tree: Option<Retained<NSString>> = unsafe { msg_send![&*top, _subtreeDescription] };
        if let Some(tree) = tree {
            out += "== window tree\n";
            out += &tree.to_string();
        }
    }
    let style: isize = unsafe { msg_send![control, segmentStyle] };
    let shape: isize = unsafe { msg_send![control, borderShape] };
    let size: usize = unsafe { msg_send![control, controlSize] };
    out += &format!("\n== segmentStyle {style} borderShape {shape} controlSize {size}\n");
    for class in [
        "NSSegmentedControl",
        "NSSegmentedCell",
        "NSToolbarItemViewer",
        "NSToolbarItem",
    ] {
        let Some(cls) = AnyClass::get(&std::ffi::CString::new(class).unwrap()) else {
            continue;
        };
        out += &format!("== methods of {class}\n");
        for method in cls.instance_methods().iter() {
            let name = method.name().name().to_string_lossy().into_owned();
            let lower = name.to_lowercase();
            if [
                "toolbar",
                "glass",
                "style",
                "select",
                "lens",
                "hover",
                "bezel",
                "material",
                "appearance",
                "context",
                "platter",
            ]
            .iter()
            .any(|word| lower.contains(word))
            {
                out += &format!("  {name}\n");
            }
        }
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if let Some(root) = exe.ancestors().find(|dir| dir.join("Cargo.toml").exists()) {
        let _ = std::fs::write(root.join("target/lab-toolbar-dump.txt"), out);
    }
}
