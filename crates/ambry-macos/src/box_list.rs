//! The popover under the box's name, laid out like Freeform's share popover: the open box
//! on top, then a row per box with a rename button, and "New Box" at the end.

use std::cell::RefCell;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{AllocAnyThread, MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSAlert, NSBox, NSBoxType, NSButton, NSClickGestureRecognizer, NSColor, NSFont, NSImageScaling,
    NSImageView, NSPopover, NSPopoverBehavior, NSTextField, NSTrackingArea, NSTrackingAreaOptions,
    NSView, NSViewController,
};
use objc2_foundation::{NSPoint, NSRect, NSRectEdge, NSSize, NSString};

use crate::native::Target;
use crate::toolbar::{
    BoxLabel, Hover, ToolbarEvent, box_color, glyph_image, place_symbol, sized_symbol,
};

const WIDTH: f64 = 260.;
const HEADER_TILE: f64 = 40.;
const ROW_HEIGHT: f64 = 26.;
const ROW_TILE: f64 = 18.;
/// How far the hover highlight sits from the popover's sides.
const ROW_INSET: f64 = 8.;
const SIDE: f64 = 16.;
const NAME_LEFT: f64 = SIDE + ROW_TILE + 9.;
/// `NSAlertFirstButtonReturn`.
const FIRST_BUTTON: isize = 1000;

pub(crate) struct BoxList {
    pub(crate) popover: Retained<NSPopover>,
    /// Targets, hover watchers and recognizers of the rows shown, which AppKit does not
    /// retain.
    parts: RefCell<Vec<Retained<NSObject>>>,
}

impl BoxList {
    pub(crate) fn new(mtm: MainThreadMarker) -> Self {
        let popover = NSPopover::new(mtm);
        popover.setBehavior(NSPopoverBehavior::Transient);
        Self {
            popover,
            parts: RefCell::new(Vec::new()),
        }
    }

    pub(crate) fn close(&self) {
        unsafe { self.popover.performClose(None) };
    }

    /// Shows the list under `anchor`: `boxes` with `current` on top.
    pub(crate) fn show(
        &self,
        mtm: MainThreadMarker,
        anchor: &NSView,
        boxes: &[BoxLabel],
        current: Option<usize>,
        on_event: Rc<dyn Fn(ToolbarEvent)>,
    ) {
        let mut parts: Vec<Retained<NSObject>> = Vec::new();
        let header_height = 14. + HEADER_TILE + 12.;
        let list_height = 8. + boxes.len() as f64 * ROW_HEIGHT + 8.;
        let footer_height = 8. + ROW_HEIGHT + 8.;
        let height = header_height + 1. + list_height + 1. + footer_height;
        let view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, height)),
        );
        // AppKit counts y from the bottom; `y` walks down from the top.
        let mut y = height - 14. - HEADER_TILE;

        // The open box: its icon, name and item count.
        if let Some(label) = current.and_then(|index| boxes.get(index)) {
            let tile = icon_tile(mtm, label, HEADER_TILE, 9., 22.);
            tile.setFrameOrigin(NSPoint::new(SIDE, y));
            view.addSubview(&tile);
            let name = label_field(mtm, &label.name, 15., true);
            let count = label_field(mtm, &items_text(label.items), 12., false);
            count.setTextColor(Some(&NSColor::secondaryLabelColor()));
            let left = SIDE + HEADER_TILE + 12.;
            let text_width = WIDTH - left - SIDE;
            name.setFrame(NSRect::new(
                NSPoint::new(left, y + HEADER_TILE / 2. - 1.),
                NSSize::new(text_width, 20.),
            ));
            count.setFrame(NSRect::new(
                NSPoint::new(left, y + HEADER_TILE / 2. - 17.),
                NSSize::new(text_width, 16.),
            ));
            view.addSubview(&name);
            view.addSubview(&count);
        }
        y -= 12.;

        y -= 1.;
        view.addSubview(&separator(mtm, y));
        y -= 8.;

        for (index, label) in boxes.iter().enumerate() {
            y -= ROW_HEIGHT;
            let row = Row::new(mtm, y, &mut parts);
            let tile = icon_tile(mtm, label, ROW_TILE, 5., 10.);
            tile.setFrameOrigin(NSPoint::new(SIDE - ROW_INSET, (ROW_HEIGHT - ROW_TILE) / 2.));
            row.view.addSubview(&tile);
            let name = label_field(mtm, &label.name, 13., current == Some(index));
            let count = label_field(mtm, &label.items.to_string(), 13., false);
            count.setTextColor(Some(&NSColor::secondaryLabelColor()));
            count.setAlignment(objc2_app_kit::NSTextAlignment::Right);

            let rename = unsafe {
                NSButton::buttonWithTitle_target_action(&NSString::new(), None, None, mtm)
            };
            rename.setImage(sized_symbol("pencil", "Rename", 12.).as_deref());
            rename.setImagePosition(objc2_app_kit::NSCellImagePosition::ImageOnly);
            rename.setBordered(false);
            rename.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
            rename.setToolTip(Some(&NSString::from_str("Rename")));
            let rename_frame = NSRect::new(
                NSPoint::new(row.width() - 8. - 22., (ROW_HEIGHT - 22.) / 2.),
                NSSize::new(22., 22.),
            );
            rename.setFrame(rename_frame);
            let rename_target = Target::new(mtm, {
                let popover = self.popover.clone();
                let on_event = on_event.clone();
                let old = label.name.clone();
                move |_| {
                    unsafe { popover.performClose(None) };
                    if let Some(name) = ask_name(mtm, &old) {
                        on_event(ToolbarEvent::RenameBox(index, name));
                    }
                }
            });
            unsafe {
                rename.setTarget(Some(&rename_target));
                rename.setAction(Some(sel!(fire:)));
            }
            parts.push(Retained::into_super(rename_target));

            let text_left = NAME_LEFT - ROW_INSET;
            let count_width = 40.;
            let count_right = rename_frame.origin.x - 6.;
            count.setFrame(NSRect::new(
                NSPoint::new(count_right - count_width, (ROW_HEIGHT - 17.) / 2.),
                NSSize::new(count_width, 17.),
            ));
            name.setFrame(NSRect::new(
                NSPoint::new(text_left, (ROW_HEIGHT - 17.) / 2.),
                NSSize::new(count_right - count_width - 6. - text_left, 17.),
            ));
            row.view.addSubview(&name);
            row.view.addSubview(&count);
            row.view.addSubview(&rename);
            row.on_click(mtm, &mut parts, Some(rename_frame), {
                let popover = self.popover.clone();
                let on_event = on_event.clone();
                move || {
                    unsafe { popover.performClose(None) };
                    on_event(ToolbarEvent::OpenBox(index));
                }
            });
            view.addSubview(&row.view);
        }
        y -= 8.;

        y -= 1.;
        view.addSubview(&separator(mtm, y));
        y -= 8. + ROW_HEIGHT;

        let row = Row::new(mtm, y, &mut parts);
        let plus = NSImageView::initWithFrame(NSImageView::alloc(mtm), NSRect::ZERO);
        plus.setImageScaling(NSImageScaling::ScaleNone);
        plus.setContentTintColor(Some(&NSColor::labelColor()));
        let plus_box = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(SIDE - ROW_INSET, (ROW_HEIGHT - ROW_TILE) / 2.),
                NSSize::new(ROW_TILE, ROW_TILE),
            ),
        );
        place_symbol(
            &plus,
            sized_symbol("plus", "New Box", 13.).as_deref(),
            ROW_TILE,
        );
        plus_box.addSubview(&plus);
        row.view.addSubview(&plus_box);
        let title = label_field(mtm, "New Box", 13., false);
        title.setFrame(NSRect::new(
            NSPoint::new(NAME_LEFT - ROW_INSET, (ROW_HEIGHT - 17.) / 2.),
            NSSize::new(row.width() - NAME_LEFT, 17.),
        ));
        row.view.addSubview(&title);
        row.on_click(mtm, &mut parts, None, {
            let popover = self.popover.clone();
            move || {
                unsafe { popover.performClose(None) };
                on_event(ToolbarEvent::NewBox);
            }
        });
        view.addSubview(&row.view);

        let controller =
            NSViewController::initWithNibName_bundle(NSViewController::alloc(mtm), None, None);
        controller.setView(&view);
        self.popover.setContentViewController(Some(&controller));
        self.popover.setContentSize(NSSize::new(WIDTH, height));
        *self.parts.borrow_mut() = parts;
        self.popover.showRelativeToRect_ofView_preferredEdge(
            anchor.bounds(),
            anchor,
            NSRectEdge::MaxY,
        );
    }
}

/// A row of the list, highlighted under the pointer like the rows of a share popover.
struct Row {
    view: Retained<NSView>,
}

impl Row {
    fn new(mtm: MainThreadMarker, y: f64, parts: &mut Vec<Retained<NSObject>>) -> Self {
        let view = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(
                NSPoint::new(ROW_INSET, y),
                NSSize::new(WIDTH - ROW_INSET * 2., ROW_HEIGHT),
            ),
        );
        view.setWantsLayer(true);
        if let Some(layer) = view.layer() {
            layer.setCornerRadius(6.);
        }
        let hover = Hover::new(mtm, {
            let view = view.clone();
            move |inside| {
                if let Some(layer) = view.layer() {
                    let fill = NSColor::quaternaryLabelColor().CGColor();
                    layer.setBackgroundColor(inside.then_some(&*fill));
                }
            }
        });
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
        view.addTrackingArea(&tracking);
        parts.push(Retained::into_super(hover));
        Self { view }
    }

    fn width(&self) -> f64 {
        self.view.frame().size.width
    }

    /// Calls `on_click` for clicks on the row outside `except`.
    fn on_click(
        &self,
        mtm: MainThreadMarker,
        parts: &mut Vec<Retained<NSObject>>,
        except: Option<NSRect>,
        on_click: impl Fn() + 'static,
    ) {
        let view = self.view.clone();
        let target = Target::new(mtm, move |sender| {
            if let (Some(except), Some(click)) =
                (except, sender.downcast_ref::<NSClickGestureRecognizer>())
            {
                let at = click.locationInView(Some(&view));
                let inside = at.x >= except.origin.x
                    && at.x <= except.origin.x + except.size.width
                    && at.y >= except.origin.y
                    && at.y <= except.origin.y + except.size.height;
                if inside {
                    return;
                }
            }
            on_click();
        });
        let click = unsafe {
            NSClickGestureRecognizer::initWithTarget_action(
                NSClickGestureRecognizer::alloc(mtm),
                Some(&target),
                Some(sel!(fire:)),
            )
        };
        self.view.addGestureRecognizer(&click);
        parts.push(Retained::into_super(target));
    }
}

/// A box's icon: its glyph in white on a rounded square of its color.
fn icon_tile(
    mtm: MainThreadMarker,
    label: &BoxLabel,
    size: f64,
    radius: f64,
    glyph: f64,
) -> Retained<NSView> {
    let tile = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::ZERO, NSSize::new(size, size)),
    );
    tile.setWantsLayer(true);
    if let Some(layer) = tile.layer() {
        layer.setCornerRadius(radius);
        layer.setBackgroundColor(Some(&box_color(&label.color).CGColor()));
    }
    let image = NSImageView::initWithFrame(NSImageView::alloc(mtm), NSRect::ZERO);
    image.setImageScaling(NSImageScaling::ScaleNone);
    image.setContentTintColor(Some(&NSColor::whiteColor()));
    place_symbol(&image, glyph_image(&label.symbol, glyph).as_deref(), size);
    tile.addSubview(&image);
    tile
}

fn label_field(mtm: MainThreadMarker, text: &str, size: f64, bold: bool) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    let weight = if bold {
        unsafe { objc2_app_kit::NSFontWeightSemibold }
    } else {
        unsafe { objc2_app_kit::NSFontWeightRegular }
    };
    field.setFont(Some(&NSFont::systemFontOfSize_weight(size, weight)));
    field.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
    field
}

fn items_text(items: usize) -> String {
    match items {
        1 => "1 item".into(),
        n => format!("{n} items"),
    }
}

fn separator(mtm: MainThreadMarker, y: f64) -> Retained<NSBox> {
    let line = NSBox::initWithFrame(
        NSBox::alloc(mtm),
        NSRect::new(NSPoint::new(SIDE, y), NSSize::new(WIDTH - SIDE * 2., 1.)),
    );
    line.setBoxType(NSBoxType::Separator);
    line
}

/// Asks for a new name for the box called `old` in a dialog; `None` when cancelled.
fn ask_name(mtm: MainThreadMarker, old: &str) -> Option<String> {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str("Rename Box"));
    alert.setInformativeText(&NSString::from_str(&format!(
        "Enter a new name for “{old}”."
    )));
    alert.addButtonWithTitle(&NSString::from_str("Rename"));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    let field = NSTextField::textFieldWithString(&NSString::from_str(old), mtm);
    field.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(240., 24.)));
    alert.setAccessoryView(Some(&field));
    alert.window().setInitialFirstResponder(Some(&field));
    if alert.runModal() != FIRST_BUTTON {
        return None;
    }
    let name = field.stringValue().to_string().trim().to_string();
    (!name.is_empty() && name != old).then_some(name)
}
