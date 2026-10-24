//! The sheet that renames a box: it slides down from the window's toolbar with the box's
//! icon on top, a name field and two buttons, like the sheets Reminders shows for a list.

use std::cell::RefCell;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, NSObject};
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, Message, msg_send, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSColor, NSControlSize, NSEvent, NSEventMask, NSFont,
    NSTextAlignment, NSTextField, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::NSMutableArray;
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::CALayer;

use crate::box_list::icon_tile;
use crate::native::Target;
use crate::toolbar::{BoxLabel, Hover, bouncy_spring, box_color};

const WIDTH: f64 = 280.;
const SIDE: f64 = 20.;
const TOP: f64 = 24.;
const TILE: f64 = 44.;
const TILE_RADIUS: f64 = 10.;
const GLYPH: f64 = 21.;
/// How much the icon grows under the pointer, and how long its spring takes.
const HOVER_SCALE: f64 = 1.08;
const HOVER_DURATION: f64 = 0.35;
/// Between the icon and the title, the title and its note, the note and the field, and the
/// field and the buttons: tight inside a group, looser between groups.
const ICON_GAP: f64 = 12.;
const NOTE_GAP: f64 = 3.;
const FIELD_GAP: f64 = 16.;
const BUTTONS_GAP: f64 = 16.;
const BUTTON_SPACING: f64 = 8.;

thread_local! {
    /// Watches for clicks on the window under the sheet while it is open.
    static MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    /// The last sheet shown, and the targets of its buttons and the icon's hover watcher,
    /// which AppKit does not retain.
    static OPEN: RefCell<Option<(Retained<NSWindow>, Vec<Retained<NSObject>>)>> =
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

    // The icon is lit from above like an app icon: a gloss over its top half, a hairline
    // rim, and a shadow in its own darker color that it seems to float on.
    let tile = icon_tile(mtm, label, TILE, TILE_RADIUS, GLYPH);
    tile.setFrameOrigin(NSPoint::new((WIDTH - TILE) / 2., y));
    let mut parts: Vec<Retained<NSObject>> = Vec::new();
    if let Some(layer) = tile.layer() {
        give_volume(&layer, &label.color);
        // Grow about the middle: AppKit pins a view's layer at its corner.
        let frame = tile.frame();
        layer.setAnchorPoint(NSPoint::new(0.5, 0.5));
        layer.setPosition(NSPoint::new(
            frame.origin.x + frame.size.width / 2.,
            frame.origin.y + frame.size.height / 2.,
        ));
        let hover = Hover::new(mtm, move |inside| lift(&layer, inside));
        let tracking = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                NSTrackingAreaOptions::MouseEnteredAndExited
                    | NSTrackingAreaOptions::ActiveAlways
                    | NSTrackingAreaOptions::InVisibleRect,
                Some(&hover),
                None,
            )
        };
        tile.addTrackingArea(&tracking);
        parts.push(Retained::into_super(hover));
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
            if let Some(monitor) = MONITOR.with_borrow_mut(Option::take) {
                unsafe { NSEvent::removeMonitor(&monitor) };
            }
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
    parts.push(Retained::into_super(cancel_target));
    parts.push(Retained::into_super(rename_target));
    OPEN.with_borrow_mut(|open| *open = Some((sheet.clone(), parts)));
    parent.beginSheet_completionHandler(&sheet, None);
    // A click on the window under the sheet cancels it, as a click outside a popover does.
    // The press goes to Cancel on the next turn of the run loop, after this event is done.
    let handler = block2::RcBlock::new({
        let parent = parent.clone();
        let cancel = cancel.clone();
        move |event: NonNull<NSEvent>| -> *mut NSEvent {
            let event = unsafe { event.as_ref() };
            let on_parent = event
                .window(mtm)
                .is_some_and(|window| std::ptr::eq(&*window, &*parent));
            if !on_parent {
                return event as *const NSEvent as *mut NSEvent;
            }
            unsafe {
                let _: () = msg_send![
                    &*cancel,
                    performSelector: sel!(performClick:),
                    withObject: std::ptr::null::<AnyObject>(),
                    afterDelay: 0.0f64
                ];
            }
            std::ptr::null_mut()
        }
    });
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown,
            &handler,
        )
    };
    MONITOR.with_borrow_mut(|slot| *slot = monitor);
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

/// Lights the tile's layer from above: a white gloss fading out by its middle, a hairline
/// rim, and a shadow of its own color, darkened.
fn give_volume(layer: &CALayer, color: &str) {
    let bounds = layer.bounds();
    if let Some(class) = AnyClass::get(c"CAGradientLayer") {
        let clear = NSColor::colorWithWhite_alpha(1., 0.).CGColor();
        let shine = NSColor::colorWithWhite_alpha(1., 0.26).CGColor();
        let colors = NSMutableArray::<AnyObject>::new();
        for color in [&clear, &shine] {
            // CGColor is toll-free bridged to an Objective-C object.
            let object: &AnyObject = unsafe { &*((&**color) as *const _ as *const AnyObject) };
            colors.addObject(object);
        }
        let locations = objc2_foundation::NSArray::from_retained_slice(&[
            objc2_foundation::NSNumber::new_f64(0.45),
            objc2_foundation::NSNumber::new_f64(1.),
        ]);
        unsafe {
            let gloss: Retained<AnyObject> = msg_send![class, layer];
            let _: () = msg_send![&*gloss, setFrame: bounds];
            let _: () = msg_send![&*gloss, setCornerRadius: layer.cornerRadius()];
            let _: () = msg_send![&*gloss, setColors: &*colors];
            let _: () = msg_send![&*gloss, setLocations: &*locations];
            // Above the fill, under the glyph.
            let _: () = msg_send![layer, insertSublayer: &*gloss, atIndex: 1u32];
        }
    }
    layer.setBorderWidth(0.5);
    layer.setBorderColor(Some(&NSColor::colorWithWhite_alpha(1., 0.22).CGColor()));
    let shadow = box_color(color)
        .blendedColorWithFraction_ofColor(0.45, &NSColor::blackColor())
        .unwrap_or_else(NSColor::blackColor);
    layer.setShadowColor(Some(&shadow.CGColor()));
    layer.setShadowOpacity(0.5);
    layer.setShadowRadius(REST_SHADOW);
    layer.setShadowOffset(NSSize::new(0., SHADOW_DROP));
}

/// The shadow's blur at rest and lifted under the pointer, and how far it drops.
const REST_SHADOW: f64 = 6.;
const LIFTED_SHADOW: f64 = 11.;
const SHADOW_DROP: f64 = -4.;

/// Raises the tile toward the pointer with a little bounce, or settles it back.
fn lift(layer: &CALayer, up: bool) {
    let (scale, shadow) = if up {
        (HOVER_SCALE, LIFTED_SHADOW)
    } else {
        (1., REST_SHADOW)
    };
    let bounce = if up { 0.35 } else { 0.15 };
    // Start from where the tile is on screen, so a quick in and out doesn't jump.
    let from_scale = shown(layer, "transform.scale").unwrap_or(if up { 1. } else { HOVER_SCALE });
    let from_shadow =
        shown(layer, "shadowRadius").unwrap_or(if up { REST_SHADOW } else { LIFTED_SHADOW });
    bouncy_spring(
        layer,
        "transform.scale",
        from_scale,
        scale,
        HOVER_DURATION,
        bounce,
    );
    bouncy_spring(
        layer,
        "shadowRadius",
        from_shadow,
        shadow,
        HOVER_DURATION,
        bounce,
    );
}

/// The value at `key_path` of what is on screen now, mid-animation or not.
fn shown(layer: &CALayer, key_path: &str) -> Option<f64> {
    let presented: Option<Retained<CALayer>> = unsafe { msg_send![layer, presentationLayer] };
    let value: Option<Retained<objc2_foundation::NSNumber>> = unsafe {
        msg_send![&*presented?, valueForKeyPath: &*objc2_foundation::NSString::from_str(key_path)]
    };
    value.map(|value| value.doubleValue())
}
