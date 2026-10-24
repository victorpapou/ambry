//! The sheet that renames a box: it slides down from the window's toolbar with the box's
//! icon on top, a name field and two buttons, like the sheets Reminders shows for a list.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, Message, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSColor, NSControlSize, NSFont, NSTextAlignment, NSTextField,
    NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use crate::box_list::icon_tile;
use crate::native::Target;
use crate::toolbar::BoxLabel;

const WIDTH: f64 = 300.;
const SIDE: f64 = 20.;
const TILE: f64 = 64.;
const FIELD_HEIGHT: f64 = 28.;
const BUTTON_HEIGHT: f64 = 28.;

thread_local! {
    /// The last sheet shown and the targets of its buttons, which AppKit does not retain.
    static OPEN: RefCell<Option<(Retained<NSWindow>, Vec<Retained<Target>>)>> =
        const { RefCell::new(None) };
}

/// Asks on a sheet over `parent` for a new name for the box `label`; `done` hears the name
/// unless the sheet is cancelled or the name is left as it was.
pub(crate) fn ask_name(
    mtm: MainThreadMarker,
    parent: &NSWindow,
    label: &BoxLabel,
    done: impl Fn(String) + 'static,
) {
    // The targets of a closed sheet are let go here rather than while one of them runs.
    if OPEN.with_borrow(|open| open.as_ref().is_some_and(|(sheet, _)| sheet.isVisible())) {
        return;
    }
    let height =
        SIDE + TILE + 14. + 18. + 4. + 16. + 16. + FIELD_HEIGHT + 20. + BUTTON_HEIGHT + SIDE;
    let sheet = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, height)),
            NSWindowStyleMask::Titled,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { sheet.setReleasedWhenClosed(false) };
    let view = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, height)),
    );
    // AppKit counts y from the bottom; `y` walks down from the top.
    let mut y = height - SIDE - TILE;

    let tile = icon_tile(mtm, label, TILE, 15., 32.);
    tile.setFrameOrigin(NSPoint::new((WIDTH - TILE) / 2., y));
    view.addSubview(&tile);
    y -= 14. + 18.;

    let title = centered_label(mtm, "Rename Box", NSFont::boldSystemFontOfSize(13.));
    title.setFrame(NSRect::new(
        NSPoint::new(SIDE, y),
        NSSize::new(WIDTH - SIDE * 2., 18.),
    ));
    view.addSubview(&title);
    y -= 4. + 16.;

    let note = centered_label(
        mtm,
        "Choose a name for this box.",
        NSFont::systemFontOfSize(NSFont::smallSystemFontSize()),
    );
    note.setTextColor(Some(&NSColor::secondaryLabelColor()));
    note.setFrame(NSRect::new(
        NSPoint::new(SIDE, y),
        NSSize::new(WIDTH - SIDE * 2., 16.),
    ));
    view.addSubview(&note);
    y -= 16. + FIELD_HEIGHT;

    let field = NSTextField::textFieldWithString(&NSString::from_str(&label.name), mtm);
    field.setFont(Some(&NSFont::systemFontOfSize(13.)));
    field.setAlignment(NSTextAlignment::Center);
    field.setBezelStyle(objc2_app_kit::NSTextFieldBezelStyle::RoundedBezel);
    field.setControlSize(NSControlSize::Large);
    field.setPlaceholderString(Some(&NSString::from_str("Box Name")));
    field.setFrame(NSRect::new(
        NSPoint::new(SIDE, y),
        NSSize::new(WIDTH - SIDE * 2., FIELD_HEIGHT),
    ));
    view.addSubview(&field);
    y -= 20. + BUTTON_HEIGHT;

    let parent = parent.retain();
    let close = {
        let parent = parent.clone();
        let sheet = sheet.clone();
        move || {
            parent.endSheet(&sheet);
            sheet.orderOut(None);
        }
    };
    let cancel_target = Target::new(mtm, {
        let close = close.clone();
        move |_| close()
    });
    let old = label.name.clone();
    let rename_target = Target::new(mtm, {
        let field = field.clone();
        move |_| {
            let name = field.stringValue().to_string().trim().to_string();
            close();
            if !name.is_empty() && name != old {
                done(name);
            }
        }
    });

    let button_width = (WIDTH - SIDE * 2. - 10.) / 2.;
    let cancel = button(mtm, "Cancel", "\u{1b}", &cancel_target);
    cancel.setFrame(NSRect::new(
        NSPoint::new(SIDE, y),
        NSSize::new(button_width, BUTTON_HEIGHT),
    ));
    view.addSubview(&cancel);
    let rename = button(mtm, "Rename", "\r", &rename_target);
    rename.setFrame(NSRect::new(
        NSPoint::new(SIDE + button_width + 10., y),
        NSSize::new(button_width, BUTTON_HEIGHT),
    ));
    view.addSubview(&rename);

    sheet.setContentView(Some(&view));
    sheet.setInitialFirstResponder(Some(&field));
    OPEN.with_borrow_mut(|open| *open = Some((sheet.clone(), vec![cancel_target, rename_target])));
    parent.beginSheet_completionHandler(&sheet, None);
    sheet.makeFirstResponder(Some(&field));
    if let Some(editor) = field.currentEditor() {
        unsafe { editor.selectAll(None) };
    }
}

fn centered_label(
    mtm: MainThreadMarker,
    text: &str,
    font: Retained<NSFont>,
) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setFont(Some(&font));
    label.setAlignment(NSTextAlignment::Center);
    label
}

fn button(mtm: MainThreadMarker, title: &str, key: &str, target: &Target) -> Retained<NSButton> {
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::from_str(title),
            Some(target),
            Some(sel!(fire:)),
            mtm,
        )
    };
    button.setControlSize(NSControlSize::Large);
    button.setKeyEquivalent(&NSString::from_str(key));
    button
}
