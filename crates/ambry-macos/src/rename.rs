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

const WIDTH: f64 = 280.;
const SIDE: f64 = 20.;
const TOP: f64 = 24.;
const TILE: f64 = 52.;
const TILE_RADIUS: f64 = 12.;
const GLYPH: f64 = 24.;
/// Between the icon and the title, the title and its note, the note and the field, and the
/// field and the buttons: tight inside a group, looser between groups.
const ICON_GAP: f64 = 12.;
const NOTE_GAP: f64 = 3.;
const FIELD_GAP: f64 = 16.;
const BUTTONS_GAP: f64 = 16.;
const BUTTON_SPACING: f64 = 8.;

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
    let inner = WIDTH - SIDE * 2.;
    let title = centered_label(
        mtm,
        "Rename Box",
        NSFont::systemFontOfSize_weight(13., unsafe { objc2_app_kit::NSFontWeightSemibold }),
    );
    let note = centered_label(
        mtm,
        &format!("Enter a new name for “{}”.", label.name),
        NSFont::systemFontOfSize(NSFont::smallSystemFontSize()),
    );
    note.setTextColor(Some(&NSColor::secondaryLabelColor()));
    note.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingMiddle);

    let field = NSTextField::textFieldWithString(&NSString::from_str(&label.name), mtm);
    field.setBezelStyle(objc2_app_kit::NSTextFieldBezelStyle::RoundedBezel);
    field.setControlSize(NSControlSize::Large);
    field.setFont(Some(&NSFont::systemFontOfSize(13.)));
    field.setAlignment(NSTextAlignment::Center);
    field.setPlaceholderString(Some(&NSString::from_str("Box Name")));
    if let Some(cell) = field.cell() {
        // One line that scrolls, so the text sits on the field's middle like a search field.
        cell.setUsesSingleLineMode(true);
        cell.setScrollable(true);
        cell.setWraps(false);
    }
    // The field and the labels at the height they ask for, so their text sits centred.
    let title_height = title.fittingSize().height.ceil();
    let note_height = note.fittingSize().height.ceil();
    let field_height = field.fittingSize().height.ceil();
    let button_height = 30.;
    let height = TOP
        + TILE
        + ICON_GAP
        + title_height
        + NOTE_GAP
        + note_height
        + FIELD_GAP
        + field_height
        + BUTTONS_GAP
        + button_height
        + SIDE;

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
    let mut y = height - TOP - TILE;

    // The icon sits on a soft shadow, as an app icon does, so it reads as an object rather
    // than a patch of color.
    let tile = icon_tile(mtm, label, TILE, TILE_RADIUS, GLYPH);
    tile.setFrameOrigin(NSPoint::new((WIDTH - TILE) / 2., y));
    if let Some(layer) = tile.layer() {
        layer.setShadowColor(Some(&NSColor::blackColor().CGColor()));
        layer.setShadowOpacity(0.18);
        layer.setShadowRadius(6.);
        layer.setShadowOffset(NSSize::new(0., -2.));
    }
    view.addSubview(&tile);

    y -= ICON_GAP + title_height;
    title.setFrame(NSRect::new(
        NSPoint::new(SIDE, y),
        NSSize::new(inner, title_height),
    ));
    view.addSubview(&title);

    y -= NOTE_GAP + note_height;
    note.setFrame(NSRect::new(
        NSPoint::new(SIDE, y),
        NSSize::new(inner, note_height),
    ));
    view.addSubview(&note);

    y -= FIELD_GAP + field_height;
    field.setFrame(NSRect::new(
        NSPoint::new(SIDE, y),
        NSSize::new(inner, field_height),
    ));
    view.addSubview(&field);
    y -= BUTTONS_GAP + button_height;

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

    let button_width = (inner - BUTTON_SPACING) / 2.;
    let cancel = button(mtm, "Cancel", "\u{1b}", &cancel_target);
    cancel.setFrame(NSRect::new(
        NSPoint::new(SIDE, y),
        NSSize::new(button_width, button_height),
    ));
    view.addSubview(&cancel);
    let rename = button(mtm, "Rename", "\r", &rename_target);
    rename.setFrame(NSRect::new(
        NSPoint::new(SIDE + button_width + BUTTON_SPACING, y),
        NSSize::new(button_width, button_height),
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
