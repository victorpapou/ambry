//! A throwaway window with switch variants side by side, for picking the one that matches
//! Xcode's inspector tabs. Opened with `--lab`.

use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, Message, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSControlBorderShape, NSControlSize, NSGlassEffectView, NSImage,
    NSSegmentStyle, NSSegmentSwitchTracking, NSSegmentedControl, NSTabPosition, NSTabView,
    NSTabViewBorderType, NSTabViewItem, NSTabViewType, NSTextField, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{NSArray, NSData, NSPoint, NSRect, NSSize, NSString};

const ROW: f64 = 56.;
const CONTROL_X: f64 = 240.;

pub fn open_switch_lab() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let rows: Vec<(&str, Retained<NSView>)> = vec![
        (
            "A current",
            segmented(mtm, |s| {
                s.setControlSize(NSControlSize::Large);
                if glass() {
                    s.setBorderShape(NSControlBorderShape::Capsule);
                }
            }),
        ),
        ("B default", segmented(mtm, |_| {})),
        (
            "C large",
            segmented(mtm, |s| s.setControlSize(NSControlSize::Large)),
        ),
        (
            "D style Separated",
            segmented(mtm, |s| s.setSegmentStyle(NSSegmentStyle::Separated)),
        ),
        (
            "E style Capsule",
            segmented(mtm, |s| s.setSegmentStyle(NSSegmentStyle::Capsule)),
        ),
        (
            "F style RoundRect",
            segmented(mtm, |s| s.setSegmentStyle(NSSegmentStyle::RoundRect)),
        ),
        (
            "G style TexturedRounded",
            segmented(mtm, |s| s.setSegmentStyle(NSSegmentStyle::TexturedRounded)),
        ),
        (
            "H glass + default",
            in_glass(mtm, segmented(mtm, |_| {}).as_ref()),
        ),
        (
            "I glass + Separated",
            in_glass(
                mtm,
                segmented(mtm, |s| s.setSegmentStyle(NSSegmentStyle::Separated)).as_ref(),
            ),
        ),
        ("J tab view, images", tabs(mtm, true)),
        ("K tab view, labels", tabs(mtm, false)),
        ("N tab control, capsule", {
            let control = crate::native::tab_segmented_control(&[
                ("number", "Canvas"),
                ("list.bullet", "List"),
            ])
            .unwrap();
            Retained::into_super(Retained::into_super(control))
        }),
        ("O tab control, square", {
            let control = crate::native::tab_segmented_control(&[
                ("number", "Canvas"),
                ("list.bullet", "List"),
            ])
            .unwrap();
            if glass() {
                control.setBorderShape(objc2_app_kit::NSControlBorderShape::Automatic);
            }
            Retained::into_super(Retained::into_super(control))
        }),
        ("M labels, no border", tabs_without_border(mtm)),
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

fn segmented(mtm: MainThreadMarker, style: impl FnOnce(&NSSegmentedControl)) -> Retained<NSView> {
    let images = NSArray::from_retained_slice(&icons());
    let s = unsafe {
        NSSegmentedControl::segmentedControlWithImages_trackingMode_target_action(
            &images,
            NSSegmentSwitchTracking::SelectOne,
            None,
            None,
            mtm,
        )
    };
    style(&s);
    s.setSelectedSegment(0);
    let size = s.fittingSize();
    s.setFrame(rect(0., 0., size.width, size.height));
    Retained::into_super(Retained::into_super(s))
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

fn tabs(mtm: MainThreadMarker, images: bool) -> Retained<NSView> {
    let view = NSTabView::initWithFrame(NSTabView::alloc(mtm), rect(0., 0., 240., 40.));
    view.setTabViewType(NSTabViewType::TopTabsBezelBorder);
    for (i, (name, image)) in ["Canvas", "List", "Help"].iter().zip(icons()).enumerate() {
        let item = unsafe {
            NSTabViewItem::initWithIdentifier(
                NSTabViewItem::alloc(),
                Some(&NSString::from_str(&i.to_string())),
            )
        };
        if images {
            item.setLabel(&NSString::from_str(""));
            item.setImage(Some(&image));
        } else {
            item.setLabel(&NSString::from_str(name));
        }
        view.addTabViewItem(&item);
    }
    Retained::into_super(view)
}

fn tabs_without_border(mtm: MainThreadMarker) -> Retained<NSView> {
    let view = NSTabView::initWithFrame(NSTabView::alloc(mtm), rect(0., 0., 240., 36.));
    view.setTabPosition(NSTabPosition::Top);
    view.setTabViewBorderType(NSTabViewBorderType::None);
    for name in ["Canvas", "List"] {
        let item = unsafe {
            NSTabViewItem::initWithIdentifier(
                NSTabViewItem::alloc(),
                Some(&NSString::from_str(name)),
            )
        };
        item.setLabel(&NSString::from_str(name));
        view.addTabViewItem(&item);
    }
    view.setFrameSize(NSSize::new(view.minimumSize().width, 36.));
    Retained::into_super(view)
}

fn icons() -> Vec<Retained<NSImage>> {
    let frame = {
        let data = NSData::with_bytes(include_bytes!("../assets/frame@3x.png"));
        let image = NSImage::initWithData(NSImage::alloc(), &data).unwrap();
        image.setSize(NSSize::new(18., 18.));
        image.setTemplate(true);
        image
    };
    let list = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str("list.bullet"),
        Some(&NSString::from_str("List")),
    )
    .unwrap();
    let help = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str("questionmark.circle"),
        Some(&NSString::from_str("Help")),
    )
    .unwrap();
    vec![frame, list, help]
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}
