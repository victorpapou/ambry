//! A throwaway window with switch variants side by side, for picking the one that matches
//! Xcode's inspector tabs. Opened with `--lab`.

use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::{MainThreadMarker, MainThreadOnly, Message, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSGlassEffectContainerView, NSGlassEffectView,
    NSGlassEffectViewStyle, NSImage, NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

const ROW: f64 = 56.;
const CONTROL_X: f64 = 240.;

pub fn open_switch_lab() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let rows: Vec<(&str, Retained<NSView>)> = vec![
        ("R glass + tabs", in_glass(mtm, &icon_tabs(mtm, 40.))),
        ("S glass + tabs, no bg", in_glass(mtm, &bare_tabs(mtm))),
        (
            "T container + glass + tabs",
            in_container(mtm, &in_glass(mtm, &bare_tabs(mtm))),
        ),
        ("U clear glass + tabs", {
            let view = in_glass(mtm, &bare_tabs(mtm));
            if let Ok(glass) = view.clone().downcast::<NSGlassEffectView>() {
                glass.setStyle(NSGlassEffectViewStyle::Clear);
            }
            view
        }),
        ("P tab view, icons", icon_tabs(mtm, 0.)),
        ("Q tab view, icons, wide", icon_tabs(mtm, 40.)),
    ];

    let height = ROW * rows.len() as f64 + 20.;
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect(0., 0., 520., height),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("Switch lab"));
    window.setBackgroundColor(Some(&NSColor::whiteColor()));
    let content = window.contentView().unwrap();
    for (i, (name, control)) in rows.iter().enumerate() {
        let y = height - 10. - ROW * (i as f64 + 1.);
        let label = NSTextField::labelWithString(&NSString::from_str(name), mtm);
        label.setFrame(rect(20., y + 20., 210., 20.));
        content.addSubview(&label);
        let size = control.frame().size;
        control.setFrame(rect(
            CONTROL_X,
            y + (ROW - size.height) / 2.,
            size.width,
            size.height,
        ));
        content.addSubview(control);
    }
    window.center();
    window.makeKeyAndOrderFront(None);
    dump(&rows);
    std::mem::forget(window);
    crate::lab_toolbar::open_toolbar_lab(mtm);
}

/// Writes each variant's view tree and the methods of AppKit's private classes in it to
/// `target/lab-dump.txt`, to see how AppKit builds the controls.
fn dump(rows: &[(&str, Retained<NSView>)]) {
    let mut out = String::new();
    let mut classes: std::collections::BTreeSet<String> =
        ["NSTabView", "NSTabViewItem"].map(String::from).into();
    for (name, view) in rows {
        out += &format!("== {name}\n");
        let tree: Option<Retained<NSString>> = unsafe { msg_send![&**view, _subtreeDescription] };
        if let Some(tree) = tree {
            out += &tree.to_string();
            out += "\n";
        }
        collect_classes(view, &mut classes);
    }
    for class in classes {
        let Some(cls) = AnyClass::get(&std::ffi::CString::new(class.clone()).unwrap()) else {
            continue;
        };
        out += &format!("== class {class}");
        if let Some(superclass) = cls.superclass() {
            out += &format!(" : {}", superclass.name().to_string_lossy());
        }
        out += "\n";
        for method in cls.instance_methods().iter() {
            out += &format!("  {}\n", method.name().name().to_string_lossy());
        }
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if let Some(root) = exe.ancestors().find(|dir| dir.join("Cargo.toml").exists()) {
        let _ = std::fs::write(root.join("target/lab-dump.txt"), out);
    }
}

fn collect_classes(view: &NSView, classes: &mut std::collections::BTreeSet<String>) {
    let name = view.class().name().to_string_lossy().into_owned();
    if name.starts_with('_') || name.contains("Tab") || name.contains("Segment") {
        classes.insert(name);
    }
    for child in view.subviews().iter() {
        collect_classes(&child, classes);
    }
}

fn glass() -> bool {
    AnyClass::get(c"NSGlassEffectView").is_some()
}

fn in_glass(mtm: MainThreadMarker, inner: &NSView) -> Retained<NSView> {
    let size = inner.frame().size;
    if !glass() {
        return inner.retain();
    }
    let g = NSGlassEffectView::initWithFrame(
        NSGlassEffectView::alloc(mtm),
        rect(0., 0., size.width, size.height),
    );
    g.setCornerRadius(size.height / 2.);
    g.setContentView(Some(inner));
    Retained::into_super(g)
}

fn icon_tabs(mtm: MainThreadMarker, item_width: f64) -> Retained<NSView> {
    let icons = [("number", "Canvas"), ("list.bullet", "List")];
    let images = crate::native::icon_images(&icons).unwrap();
    Retained::into_super(crate::native::icon_tab_view(
        mtm,
        icons.len(),
        &images,
        item_width,
    ))
}

/// Icon tabs whose NSTabView draws no background of its own.
fn bare_tabs(mtm: MainThreadMarker) -> Retained<NSView> {
    let icons = [("number", "Canvas"), ("list.bullet", "List")];
    let images = crate::native::icon_images(&icons).unwrap();
    let tabs = crate::native::icon_tab_view(mtm, icons.len(), &images, 40.);
    tabs.setDrawsBackground(false);
    Retained::into_super(tabs)
}

fn in_container(mtm: MainThreadMarker, inner: &NSView) -> Retained<NSView> {
    if !glass() {
        return inner.retain();
    }
    let container = NSGlassEffectContainerView::initWithFrame(
        NSGlassEffectContainerView::alloc(mtm),
        inner.frame(),
    );
    container.setContentView(Some(inner));
    Retained::into_super(container)
}

pub(crate) fn symbol(name: &str, description: &str) -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        Some(&NSString::from_str(description)),
    )
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}
