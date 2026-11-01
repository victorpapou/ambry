//! The canvas as Finder's icon view: a real NSScrollView, so pinching, smart zoom, inertia,
//! the rubber band and the scroll bars are AppKit's own, over a document view that draws the
//! dot grid and each item the way Finder draws an icon and its name.
//!
//! macOS has no public icon view to borrow, so the document view draws the icons itself:
//! Finder's icon for every file from NSWorkspace, a grey plate behind a selected icon and its
//! name in an accent capsule. Icons grow with the zoom; names keep their size on screen.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
};
use objc2_app_kit::{
    NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSBezierPath,
    NSColor, NSCompositingOperation, NSDragOperation, NSDraggingDestination, NSDraggingInfo,
    NSEvent, NSEventModifierFlags, NSFont, NSFontAttributeName, NSForegroundColorAttributeName,
    NSImage, NSLineBreakMode, NSMutableParagraphStyle, NSParagraphStyleAttributeName,
    NSPasteboardTypeFileURL, NSScrollView, NSStringDrawingOptions, NSStringNSExtendedStringDrawing,
    NSTextAlignment, NSView, NSWorkspace,
};
use objc2_foundation::{NSArray, NSDictionary, NSPoint, NSRect, NSSize, NSString, NSURL};

use crate::native::NativeView;

/// Width of the slot an item takes: its icon is centred in it and its name wraps in it.
/// Item positions are the slot's top-left, as the GPUI canvas kept them.
const SLOT: f64 = 128.;
const ICON: f64 = 64.;
/// From the slot's top to the icon's, and from the icon's bottom to the name's.
const ICON_TOP: f64 = 8.;
const NAME_GAP: f64 = 6.;
/// The name: its size on screen, how wide it may wrap, and the capsule's padding.
const NAME_SIZE: f64 = 12.;
const NAME_WIDTH: f64 = 112.;
const NAME_PAD_X: f64 = 5.;
const NAME_PAD_Y: f64 = 1.;
/// The plate behind a selected icon reaches this far past it, with these corners.
const PLATE_OUTSET: f64 = 5.;
const PLATE_RADIUS: f64 = 6.;
/// Freeform's grid: dots this far apart at actual size, this big on screen.
const DOT_SPACING: f64 = 25.;
const DOT_RADIUS: f64 = 1.1;
/// Room around the items the canvas scrolls through, in canvas points.
const MARGIN: f64 = 600.;
pub const MIN_ZOOM: f64 = 0.3;
pub const MAX_ZOOM: f64 = 4.;
/// A press has to move this far, on screen, before it drags.
const DRAG_SLOP: f64 = 3.;
/// The largest picture shown as its own thumbnail, in bytes; bigger ones get Finder's icon.
const MAX_THUMBNAIL: u64 = 20 * 1024 * 1024;

/// One item as the canvas shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct IconItem {
    pub id: u64,
    pub path: PathBuf,
    pub name: String,
    /// The slot's top-left in canvas points.
    pub x: f64,
    pub y: f64,
    /// The file is gone from disk: drawn faded.
    pub missing: bool,
}

/// What happened on the canvas.
#[derive(Clone, Debug, PartialEq)]
pub enum IconEvent {
    /// These items are selected now, in the order they were picked.
    Select(Vec<u64>),
    /// Items were dragged here: each id with its slot's new top-left.
    Move(Vec<(u64, f64, f64)>),
    /// Double click or Return.
    Open(Vec<u64>),
    /// Backspace or Delete: take these out of the box.
    Remove(Vec<u64>),
    /// Files dropped from Finder, the first one's slot centred at this canvas point.
    Drop(Vec<PathBuf>, f64, f64),
    /// The space bar: Quick Look on the selection.
    QuickLook,
    /// The zoom changed to this, 1 for actual size.
    Zoom(f64),
}

enum Press {
    /// On an item: dragging moves the selection. Where it started, in canvas points, and
    /// where each selected item was.
    Item {
        start: NSPoint,
        origins: Vec<(u64, f64, f64)>,
        moved: bool,
    },
    /// On the empty canvas: dragging draws a rectangle that picks what it touches. What was
    /// selected before, kept when Shift or Command is held.
    Marquee {
        start: NSPoint,
        now: NSPoint,
        before: Vec<u64>,
    },
}

#[derive(Default)]
struct State {
    items: Vec<IconItem>,
    selected: Vec<u64>,
    press: Option<Press>,
    icons: HashMap<PathBuf, Retained<NSImage>>,
    /// The canvas point at the document view's top-left: the document grows to the items in
    /// every direction, and this keeps the items still while it does.
    origin: (f64, f64),
    /// Files hovering over the canvas.
    dropping: bool,
}

pub(crate) struct Ivars {
    state: RefCell<State>,
    on_event: Box<dyn Fn(IconEvent)>,
    zoom: Cell<f64>,
}

define_class!(
    // The scroll view's document: the dots and the items, drawn in canvas points.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryIconDocument"]
    #[ivars = Ivars]
    pub(crate) struct Document;

    unsafe impl NSObjectProtocol for Document {}

    unsafe impl NSDraggingDestination for Document {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            self.ivars().state.borrow_mut().dropping = true;
            self.setNeedsDisplay(true);
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, _sender: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            self.ivars().state.borrow_mut().dropping = false;
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> objc2::runtime::Bool {
            self.ivars().state.borrow_mut().dropping = false;
            self.setNeedsDisplay(true);
            let paths = dropped_paths(sender);
            if paths.is_empty() {
                return false.into();
            }
            let at = self.convertPoint_fromView(sender.draggingLocation(), None);
            let (x, y) = self.to_canvas(at);
            (self.ivars().on_event)(IconEvent::Drop(paths, x, y));
            true.into()
        }
    }

    impl Document {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        // The selection turns from blue to grey when the canvas loses the keyboard.
        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            self.setNeedsDisplay(true);
            true
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            self.setNeedsDisplay(true);
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            self.draw(dirty);
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.on_mouse_down(event);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.on_mouse_dragged(event);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            self.on_mouse_up();
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if !self.on_key_down(event) {
                unsafe {
                    let _: () = msg_send![super(self), keyDown: event];
                }
            }
        }
    }
);

impl Document {
    fn emit(&self, event: IconEvent) {
        (self.ivars().on_event)(event);
    }

    fn zoom(&self) -> f64 {
        self.ivars().zoom.get().max(0.01)
    }

    /// A point in the document view as a canvas point.
    fn to_canvas(&self, at: NSPoint) -> (f64, f64) {
        let (x, y) = self.ivars().state.borrow().origin;
        (at.x + x, at.y + y)
    }

    fn event_point(&self, event: &NSEvent) -> NSPoint {
        self.convertPoint_fromView(event.locationInWindow(), None)
    }

    /// Where an item's icon and name are drawn, in document points.
    fn frames(&self, item: &IconItem, origin: (f64, f64), name: &NameLayout) -> (NSRect, NSRect) {
        let left = item.x - origin.0;
        let top = item.y - origin.1;
        let icon = NSRect::new(
            NSPoint::new(left + (SLOT - ICON) / 2., top + ICON_TOP),
            NSSize::new(ICON, ICON),
        );
        let zoom = self.zoom();
        let width = name.size.width + NAME_PAD_X * 2. / zoom;
        let height = name.size.height + NAME_PAD_Y * 2. / zoom;
        let label = NSRect::new(
            NSPoint::new(
                left + SLOT / 2. - width / 2.,
                top + ICON_TOP + ICON + NAME_GAP / zoom,
            ),
            NSSize::new(width, height),
        );
        (icon, label)
    }

    /// The item under a document point, the topmost first.
    fn hit(&self, at: NSPoint) -> Option<u64> {
        let state = self.ivars().state.borrow();
        let zoom = self.zoom();
        state.items.iter().rev().find_map(|item| {
            let name = NameLayout::new(&item.name, zoom);
            let (icon, label) = self.frames(item, state.origin, &name);
            (contains(icon, at) || contains(label, at)).then_some(item.id)
        })
    }

    fn set_selection(&self, ids: Vec<u64>) {
        let changed = {
            let mut state = self.ivars().state.borrow_mut();
            let changed = state.selected != ids;
            state.selected = ids.clone();
            changed
        };
        if changed {
            self.setNeedsDisplay(true);
            self.emit(IconEvent::Select(ids));
        }
    }

    fn on_mouse_down(&self, event: &NSEvent) {
        if let Some(window) = self.window() {
            window.makeFirstResponder(Some(self));
        }
        let at = self.event_point(event);
        let flags = event.modifierFlags();
        let toggle = flags.contains(NSEventModifierFlags::Command)
            || flags.contains(NSEventModifierFlags::Shift);
        let selected = self.ivars().state.borrow().selected.clone();
        match self.hit(at) {
            Some(id) => {
                if event.clickCount() >= 2 && !toggle {
                    let ids = if selected.contains(&id) {
                        selected
                    } else {
                        vec![id]
                    };
                    self.emit(IconEvent::Open(ids));
                    return;
                }
                let mut picked = selected.clone();
                if toggle {
                    if let Some(index) = picked.iter().position(|picked| *picked == id) {
                        picked.remove(index);
                    } else {
                        picked.push(id);
                    }
                } else if !picked.contains(&id) {
                    picked = vec![id];
                }
                self.set_selection(picked.clone());
                if !picked.contains(&id) {
                    return;
                }
                let origins = {
                    let state = self.ivars().state.borrow();
                    state
                        .items
                        .iter()
                        .filter(|item| picked.contains(&item.id))
                        .map(|item| (item.id, item.x, item.y))
                        .collect()
                };
                let (x, y) = self.to_canvas(at);
                self.ivars().state.borrow_mut().press = Some(Press::Item {
                    start: NSPoint::new(x, y),
                    origins,
                    moved: false,
                });
            }
            None => {
                let before = if toggle { selected } else { Vec::new() };
                if !toggle {
                    self.set_selection(Vec::new());
                }
                if event.clickCount() >= 2 {
                    return;
                }
                let (x, y) = self.to_canvas(at);
                let start = NSPoint::new(x, y);
                self.ivars().state.borrow_mut().press = Some(Press::Marquee {
                    start,
                    now: start,
                    before,
                });
            }
        }
    }

    fn on_mouse_dragged(&self, event: &NSEvent) {
        let at = self.event_point(event);
        let (x, y) = self.to_canvas(at);
        let zoom = self.zoom();
        let mut pick = None;
        {
            let mut state = self.ivars().state.borrow_mut();
            let State { press, items, .. } = &mut *state;
            match press {
                Some(Press::Item {
                    start,
                    origins,
                    moved,
                }) => {
                    let (dx, dy) = (x - start.x, y - start.y);
                    if !*moved && (dx.abs().max(dy.abs()) * zoom) < DRAG_SLOP {
                        return;
                    }
                    *moved = true;
                    for (id, ox, oy) in origins.iter() {
                        if let Some(item) = items.iter_mut().find(|item| item.id == *id) {
                            item.x = ox + dx;
                            item.y = oy + dy;
                        }
                    }
                }
                Some(Press::Marquee { start, now, before }) => {
                    *now = NSPoint::new(x, y);
                    let rect = rect_between(*start, *now);
                    let mut picked = before.clone();
                    for item in items.iter() {
                        let name = NameLayout::new(&item.name, zoom);
                        let (icon, label) = self.frames(item, (0., 0.), &name);
                        if (intersects(rect, icon) || intersects(rect, label))
                            && !picked.contains(&item.id)
                        {
                            picked.push(item.id);
                        }
                    }
                    pick = Some(picked);
                }
                None => return,
            }
        }
        if let Some(picked) = pick {
            self.set_selection(picked);
        }
        self.autoscroll(event);
        self.setNeedsDisplay(true);
    }

    fn on_mouse_up(&self) {
        let press = self.ivars().state.borrow_mut().press.take();
        match press {
            Some(Press::Item {
                origins,
                moved: true,
                ..
            }) => {
                let moves: Vec<(u64, f64, f64)> = {
                    let state = self.ivars().state.borrow();
                    origins
                        .iter()
                        .filter_map(|(id, _, _)| {
                            state
                                .items
                                .iter()
                                .find(|item| item.id == *id)
                                .map(|item| (item.id, item.x.round(), item.y.round()))
                        })
                        .collect()
                };
                self.emit(IconEvent::Move(moves));
                self.fit();
            }
            Some(Press::Marquee { .. }) => self.setNeedsDisplay(true),
            _ => {}
        }
    }

    /// Handles the keys Finder's icon view handles; the rest go on to the window.
    fn on_key_down(&self, event: &NSEvent) -> bool {
        let flags = event.modifierFlags();
        let command = flags.contains(NSEventModifierFlags::Command);
        let selected = self.ivars().state.borrow().selected.clone();
        let key = event
            .charactersIgnoringModifiers()
            .map(|chars| chars.to_string())
            .unwrap_or_default();
        match (event.keyCode(), command, key.as_str()) {
            // Backspace and forward delete.
            (51 | 117, _, _) if !selected.is_empty() => {
                self.set_selection(Vec::new());
                self.emit(IconEvent::Remove(selected));
            }
            (49, false, _) => self.emit(IconEvent::QuickLook),
            (36 | 76, false, _) if !selected.is_empty() => self.emit(IconEvent::Open(selected)),
            (53, _, _) => self.set_selection(Vec::new()),
            (_, true, "a") => {
                let all = self
                    .ivars()
                    .state
                    .borrow()
                    .items
                    .iter()
                    .map(|item| item.id)
                    .collect();
                self.set_selection(all);
            }
            _ => return false,
        }
        true
    }

    /// Grows or shrinks the document to the items plus a margin, and never smaller than what
    /// is on screen, keeping everything where it is on screen.
    fn fit(&self) {
        let Some(clip) = (unsafe { self.superview() }) else {
            return;
        };
        let visible = clip.bounds();
        let (old_x, old_y) = self.ivars().state.borrow().origin;
        // What is on screen, in canvas points.
        let view_left = visible.origin.x + old_x;
        let view_top = visible.origin.y + old_y;
        let (mut left, mut top) = (view_left, view_top);
        let (mut right, mut bottom) = (
            view_left + visible.size.width,
            view_top + visible.size.height,
        );
        {
            let state = self.ivars().state.borrow();
            for item in &state.items {
                left = left.min(item.x - MARGIN);
                top = top.min(item.y - MARGIN);
                right = right.max(item.x + SLOT + MARGIN);
                bottom = bottom.max(item.y + SLOT + MARGIN);
            }
        }
        if state_is_empty(self) {
            // An empty box: the canvas's origin sits at the top-left of the window.
            left = left.min(-MARGIN / 4.);
            top = top.min(-MARGIN / 4.);
        }
        let (left, top) = (left.floor(), top.floor());
        self.ivars().state.borrow_mut().origin = (left, top);
        self.setFrameSize(NSSize::new(right.ceil() - left, bottom.ceil() - top));
        if (left, top) != (old_x, old_y) {
            // The same canvas point stays at the top-left of the screen.
            let clip: Retained<objc2_app_kit::NSClipView> =
                unsafe { Retained::cast_unchecked(clip) };
            clip.scrollToPoint(NSPoint::new(view_left - left, view_top - top));
            if let Some(scroll) = unsafe { clip.superview() } {
                let scroll: Retained<NSScrollView> = unsafe { Retained::cast_unchecked(scroll) };
                scroll.reflectScrolledClipView(&clip);
            }
        }
        self.setNeedsDisplay(true);
    }

    fn dark(&self) -> bool {
        let names =
            unsafe { NSArray::from_slice(&[NSAppearanceNameAqua, NSAppearanceNameDarkAqua]) };
        self.effectiveAppearance()
            .bestMatchFromAppearancesWithNames(&names)
            .is_some_and(|name| &*name == unsafe { NSAppearanceNameDarkAqua })
    }

    fn draw(&self, dirty: NSRect) {
        let dark = self.dark();
        let zoom = self.zoom();
        let (canvas, dot) = if dark {
            (rgb(0x1e1e1e, 1.), rgb(0x5c5c60, 1.))
        } else {
            (rgb(0xffffff, 1.), rgb(0xc7c7cc, 1.))
        };
        canvas.setFill();
        NSBezierPath::fillRect(dirty);

        let state = self.ivars().state.borrow();
        let origin = state.origin;

        // The dots stay the same size on screen and thin out as the canvas shrinks, so the
        // grid never turns into a grey wash.
        let mut spacing = DOT_SPACING;
        while spacing * zoom < DOT_SPACING / 2. {
            spacing *= 2.;
        }
        let radius = DOT_RADIUS / zoom;
        dot.setFill();
        let first_x = ((dirty.origin.x + origin.0 - radius) / spacing).ceil() as i64;
        let first_y = ((dirty.origin.y + origin.1 - radius) / spacing).ceil() as i64;
        let last_x = ((dirty.origin.x + dirty.size.width + origin.0 + radius) / spacing) as i64;
        let last_y = ((dirty.origin.y + dirty.size.height + origin.1 + radius) / spacing) as i64;
        if (last_x - first_x) * (last_y - first_y) < 200_000 {
            for row in first_y..=last_y {
                for column in first_x..=last_x {
                    let x = column as f64 * spacing - origin.0;
                    let y = row as f64 * spacing - origin.1;
                    NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
                        NSPoint::new(x - radius, y - radius),
                        NSSize::new(radius * 2., radius * 2.),
                    ))
                    .fill();
                }
            }
        }

        let focused = self.window().is_some_and(|window| {
            window.isKeyWindow()
                && window.firstResponder().is_some_and(|responder| {
                    std::ptr::eq(&*responder as *const _ as *const Self, self)
                })
        });
        let lifted: HashSet<u64> = match &state.press {
            Some(Press::Item {
                origins,
                moved: true,
                ..
            }) => origins.iter().map(|(id, _, _)| *id).collect(),
            _ => HashSet::new(),
        };
        drop(state);
        let items = self.ivars().state.borrow().items.clone();
        let selected = self.ivars().state.borrow().selected.clone();
        // Carried items on top.
        let mut order: Vec<&IconItem> = items
            .iter()
            .filter(|item| !lifted.contains(&item.id))
            .collect();
        order.extend(items.iter().filter(|item| lifted.contains(&item.id)));
        for item in order {
            let name = NameLayout::new(&item.name, zoom);
            let (icon, label) = self.frames(item, origin, &name);
            let reach = NSRect::new(
                NSPoint::new(icon.origin.x - PLATE_OUTSET, icon.origin.y - PLATE_OUTSET),
                NSSize::new(
                    icon.size.width.max(label.size.width) + PLATE_OUTSET * 2.,
                    label.origin.y + label.size.height - icon.origin.y + PLATE_OUTSET * 2.,
                ),
            );
            if !intersects(reach, expand(dirty, SLOT)) {
                continue;
            }
            let picked = selected.contains(&item.id);
            if picked {
                // Finder's plate: a soft grey rounded square behind the icon.
                let plate = NSRect::new(
                    NSPoint::new(icon.origin.x - PLATE_OUTSET, icon.origin.y - PLATE_OUTSET),
                    NSSize::new(ICON + PLATE_OUTSET * 2., ICON + PLATE_OUTSET * 2.),
                );
                if dark {
                    rgb(0xffffff, 0.12).setFill();
                } else {
                    rgb(0x000000, 0.08).setFill();
                }
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(
                    plate,
                    PLATE_RADIUS,
                    PLATE_RADIUS,
                )
                .fill();
            }
            let alpha = if item.missing { 0.4 } else { 1. };
            let alpha = if lifted.contains(&item.id) {
                alpha * 0.85
            } else {
                alpha
            };
            if let Some(image) = self.icon(&item.path) {
                let rect = fit_rect(image.size(), icon);
                unsafe {
                    image.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                        rect,
                        NSRect::ZERO,
                        NSCompositingOperation::SourceOver,
                        alpha,
                        true,
                        None,
                    );
                }
            }
            let text_color = if picked {
                let capsule = if focused {
                    NSColor::selectedContentBackgroundColor()
                } else {
                    NSColor::unemphasizedSelectedContentBackgroundColor()
                };
                capsule.setFill();
                let radius = (label.size.height / 2.).min(5. / zoom);
                NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(label, radius, radius)
                    .fill();
                if focused {
                    NSColor::alternateSelectedControlTextColor()
                } else {
                    NSColor::labelColor()
                }
            } else {
                NSColor::labelColor()
            };
            let text = NSRect::new(
                NSPoint::new(
                    label.origin.x + NAME_PAD_X / zoom,
                    label.origin.y + NAME_PAD_Y / zoom,
                ),
                name.size,
            );
            name.draw(text, &text_color.colorWithAlphaComponent(alpha));
        }

        if items.is_empty() {
            self.draw_hint();
        }

        let state = self.ivars().state.borrow();
        if let Some(Press::Marquee { start, now, .. }) = &state.press {
            let rect = rect_between(
                NSPoint::new(start.x - origin.0, start.y - origin.1),
                NSPoint::new(now.x - origin.0, now.y - origin.1),
            );
            NSColor::selectedContentBackgroundColor()
                .colorWithAlphaComponent(0.15)
                .setFill();
            NSBezierPath::fillRect(rect);
            NSColor::selectedContentBackgroundColor()
                .colorWithAlphaComponent(0.8)
                .setStroke();
            let border = NSBezierPath::bezierPathWithRect(rect);
            border.setLineWidth(1. / zoom);
            border.stroke();
        }
    }

    /// "Drop files or folders here" in the middle of what is on screen, for an empty box.
    fn draw_hint(&self) {
        let Some(clip) = (unsafe { self.superview() }) else {
            return;
        };
        let visible = clip.bounds();
        let zoom = self.zoom();
        let font = NSFont::systemFontOfSize(15. / zoom);
        let attributes = attributes(&font, &NSColor::tertiaryLabelColor());
        let text = NSString::from_str("Drop files or folders here");
        let height = (font.ascender() - font.descender()).ceil();
        let rect = NSRect::new(
            NSPoint::new(
                visible.origin.x,
                visible.origin.y + visible.size.height / 2. - height / 2.,
            ),
            NSSize::new(visible.size.width, height),
        );
        unsafe {
            text.drawWithRect_options_attributes_context(
                rect,
                NSStringDrawingOptions::UsesLineFragmentOrigin,
                Some(&attributes),
                None,
            );
        }
    }

    /// Finder's icon for a file, or the picture itself for an image, made once per path.
    fn icon(&self, path: &Path) -> Option<Retained<NSImage>> {
        if let Some(image) = self.ivars().state.borrow().icons.get(path) {
            return Some(image.clone());
        }
        let ns_path = NSString::from_str(&path.to_string_lossy());
        let picture = is_picture(path)
            && std::fs::metadata(path).is_ok_and(|meta| meta.len() <= MAX_THUMBNAIL);
        let image = picture
            .then(|| NSImage::initWithContentsOfFile(NSImage::alloc(), &ns_path))
            .flatten()
            .unwrap_or_else(|| NSWorkspace::sharedWorkspace().iconForFile(&ns_path));
        self.ivars()
            .state
            .borrow_mut()
            .icons
            .insert(path.to_path_buf(), image.clone());
        Some(image)
    }
}

fn state_is_empty(document: &Document) -> bool {
    document.ivars().state.borrow().items.is_empty()
}

/// A name laid out for drawing at the current zoom: two lines at most, cut in the middle.
struct NameLayout {
    text: Retained<NSString>,
    attributes: Retained<NSDictionary<NSString, AnyObject>>,
    font: Retained<NSFont>,
    /// The text's size in document points.
    size: NSSize,
}

impl NameLayout {
    fn new(name: &str, zoom: f64) -> Self {
        let font = NSFont::systemFontOfSize(NAME_SIZE / zoom);
        let text = NSString::from_str(name);
        let attributes = attributes(&font, &NSColor::labelColor());
        let options = NSStringDrawingOptions::UsesLineFragmentOrigin
            | NSStringDrawingOptions::TruncatesLastVisibleLine;
        let line = font.ascender() - font.descender() + font.leading();
        let most = NSSize::new(NAME_WIDTH / zoom, (line * 2.).ceil());
        let bounds = unsafe {
            text.boundingRectWithSize_options_attributes_context(
                most,
                options,
                Some(&attributes),
                None,
            )
        };
        let size = NSSize::new(
            bounds.size.width.ceil().min(most.width),
            bounds.size.height.ceil().min(most.height).max(line.ceil()),
        );
        Self {
            text,
            attributes,
            font,
            size,
        }
    }

    fn draw(&self, rect: NSRect, color: &NSColor) {
        let attributes = attributes(&self.font, color);
        let _ = &self.attributes;
        let options = NSStringDrawingOptions::UsesLineFragmentOrigin
            | NSStringDrawingOptions::TruncatesLastVisibleLine;
        unsafe {
            self.text.drawWithRect_options_attributes_context(
                rect,
                options,
                Some(&attributes),
                None,
            );
        }
    }
}

fn attributes(font: &NSFont, color: &NSColor) -> Retained<NSDictionary<NSString, AnyObject>> {
    let style = NSMutableParagraphStyle::new();
    style.setAlignment(NSTextAlignment::Center);
    style.setLineBreakMode(NSLineBreakMode::ByWordWrapping);
    unsafe {
        NSDictionary::from_slices(
            &[
                NSFontAttributeName,
                NSForegroundColorAttributeName,
                NSParagraphStyleAttributeName,
            ],
            &[
                font as &AnyObject,
                color as &AnyObject,
                &*style as &AnyObject,
            ],
        )
    }
}

/// The files on a drag from Finder.
fn dropped_paths(sender: &ProtocolObject<dyn NSDraggingInfo>) -> Vec<PathBuf> {
    let pasteboard = sender.draggingPasteboard();
    let Some(items) = pasteboard.pasteboardItems() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let url = item.stringForType(unsafe { NSPasteboardTypeFileURL })?;
            // Finder hands out file reference URLs, file:///.file/id=…; the path is behind it.
            let url = NSURL::URLWithString(&url)?;
            let url = url.filePathURL().unwrap_or(url);
            url.path().map(|path| PathBuf::from(path.to_string()))
        })
        .collect()
}

fn is_picture(path: &Path) -> bool {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .is_some_and(|ext| {
            matches!(
                ext.as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "tiff" | "bmp"
            )
        })
}

fn rgb(hex: u32, alpha: f64) -> Retained<NSColor> {
    let channel = |shift: u32| ((hex >> shift) & 0xff) as f64 / 255.;
    NSColor::colorWithSRGBRed_green_blue_alpha(channel(16), channel(8), channel(0), alpha)
}

/// `size` scaled to fit in `rect`, centred, keeping its proportions.
fn fit_rect(size: NSSize, rect: NSRect) -> NSRect {
    if size.width <= 0. || size.height <= 0. {
        return rect;
    }
    let scale = (rect.size.width / size.width).min(rect.size.height / size.height);
    let (width, height) = (size.width * scale, size.height * scale);
    NSRect::new(
        NSPoint::new(
            rect.origin.x + (rect.size.width - width) / 2.,
            rect.origin.y + (rect.size.height - height) / 2.,
        ),
        NSSize::new(width, height),
    )
}

fn contains(rect: NSRect, at: NSPoint) -> bool {
    at.x >= rect.origin.x
        && at.y >= rect.origin.y
        && at.x <= rect.origin.x + rect.size.width
        && at.y <= rect.origin.y + rect.size.height
}

fn intersects(a: NSRect, b: NSRect) -> bool {
    a.origin.x < b.origin.x + b.size.width
        && b.origin.x < a.origin.x + a.size.width
        && a.origin.y < b.origin.y + b.size.height
        && b.origin.y < a.origin.y + a.size.height
}

fn expand(rect: NSRect, by: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(rect.origin.x - by, rect.origin.y - by),
        NSSize::new(rect.size.width + by * 2., rect.size.height + by * 2.),
    )
}

fn rect_between(a: NSPoint, b: NSPoint) -> NSRect {
    NSRect::new(
        NSPoint::new(a.x.min(b.x), a.y.min(b.y)),
        NSSize::new((a.x - b.x).abs(), (a.y - b.y).abs()),
    )
}

/// The canvas in Finder's icon view style.
pub struct IconCanvas {
    native: NativeView,
    scroll: Retained<NSScrollView>,
    document: Retained<Document>,
    _observers: [Retained<AnyObject>; 2],
}

impl Drop for IconCanvas {
    fn drop(&mut self) {
        // The observers' block holds the views; let go of them with the canvas.
        let center = objc2_foundation::NSNotificationCenter::defaultCenter();
        for observer in &self._observers {
            unsafe { center.removeObserver(observer) };
        }
    }
}

impl IconCanvas {
    /// # Safety
    /// `ns_view` must point to a live `NSView`, and this must run on the main thread.
    pub unsafe fn new(
        ns_view: NonNull<c_void>,
        on_event: impl Fn(IconEvent) + 'static,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let parent: &NSView = unsafe { ns_view.cast().as_ref() };
        let frame = NSRect::new(NSPoint::ZERO, NSSize::new(400., 300.));
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setHasHorizontalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setAllowsMagnification(true);
        scroll.setMinMagnification(MIN_ZOOM);
        scroll.setMaxMagnification(MAX_ZOOM);
        scroll.setHorizontalScrollElasticity(objc2_app_kit::NSScrollElasticity::Allowed);
        scroll.setVerticalScrollElasticity(objc2_app_kit::NSScrollElasticity::Allowed);

        let document = Document::alloc(mtm).set_ivars(Ivars {
            state: RefCell::new(State::default()),
            on_event: Box::new(on_event),
            zoom: Cell::new(1.),
        });
        let document: Retained<Document> =
            unsafe { msg_send![super(document), initWithFrame: frame] };
        document
            .registerForDraggedTypes(&NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]));
        scroll.setDocumentView(Some(&document));

        // The zoom: names are redrawn at their screen size and the label shows the percent.
        let clip = scroll.contentView();
        clip.setPostsBoundsChangedNotifications(true);
        clip.setPostsFrameChangedNotifications(true);
        let observer = {
            let document = document.clone();
            let scroll = scroll.clone();
            let block =
                block2::RcBlock::new(move |_note: NonNull<objc2_foundation::NSNotification>| {
                    let zoom = scroll.magnification();
                    if (zoom - document.ivars().zoom.get()).abs() > 1e-6 {
                        document.ivars().zoom.set(zoom);
                        document.setNeedsDisplay(true);
                        (document.ivars().on_event)(IconEvent::Zoom(zoom));
                    }
                    // Zoomed out or a bigger window: the dots reach every edge.
                    let visible = scroll.contentView().bounds();
                    let frame = document.frame();
                    let covered = visible.origin.x >= frame.origin.x
                        && visible.origin.y >= frame.origin.y
                        && visible.origin.x + visible.size.width
                            <= frame.origin.x + frame.size.width + 0.5
                        && visible.origin.y + visible.size.height
                            <= frame.origin.y + frame.size.height + 0.5;
                    if !covered && document.ivars().state.borrow().press.is_none() {
                        document.fit();
                    }
                });
            let center = objc2_foundation::NSNotificationCenter::defaultCenter();
            unsafe {
                [
                    objc2_app_kit::NSViewBoundsDidChangeNotification,
                    objc2_app_kit::NSViewFrameDidChangeNotification,
                ]
                .map(|name| {
                    let observer = center.addObserverForName_object_queue_usingBlock(
                        Some(name),
                        Some(&clip),
                        None,
                        &block,
                    );
                    let observer: Retained<AnyObject> = Retained::cast_unchecked(observer);
                    observer
                })
            }
        };

        let view: Retained<NSView> = Retained::into_super(scroll.clone());
        let canvas = Self {
            native: NativeView::host(parent, view, None),
            scroll,
            document,
            _observers: observer,
        };
        Some(canvas)
    }

    pub fn native(&self) -> &NativeView {
        &self.native
    }

    /// Shows `items`, keeping the selection of those still there. Does nothing while an
    /// item is being dragged, so the drag is not reset under the pointer.
    pub fn set_items(&self, items: Vec<IconItem>) {
        {
            let mut state = self.document.ivars().state.borrow_mut();
            if state.press.is_some() || state.items == items {
                return;
            }
            let ids: HashSet<u64> = items.iter().map(|item| item.id).collect();
            state.selected.retain(|id| ids.contains(id));
            let paths: HashSet<&PathBuf> = items.iter().map(|item| &item.path).collect();
            state.icons.retain(|path, _| paths.contains(path));
            state.items = items;
        }
        self.document.fit();
    }

    /// Selects `ids` without reporting it back.
    pub fn select(&self, ids: &[u64]) {
        let mut state = self.document.ivars().state.borrow_mut();
        if state.selected != ids {
            state.selected = ids.to_vec();
            drop(state);
            self.document.setNeedsDisplay(true);
        }
    }

    /// Forgets the icon of `path`, after its file changed.
    pub fn refresh_icon(&self, path: &Path) {
        self.document.ivars().state.borrow_mut().icons.remove(path);
        self.document.setNeedsDisplay(true);
    }

    /// The zoom, 1 for actual size.
    pub fn zoom(&self) -> f64 {
        self.scroll.magnification()
    }

    /// Zooms by `factor` about the middle of what is on screen, with AppKit's animation.
    pub fn zoom_by(&self, factor: f64) {
        self.zoom_to(self.zoom() * factor);
    }

    pub fn zoom_to(&self, zoom: f64) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let clip = self.scroll.contentView();
        let visible = clip.bounds();
        let centre = NSPoint::new(
            visible.origin.x + visible.size.width / 2.,
            visible.origin.y + visible.size.height / 2.,
        );
        let animator: Retained<NSScrollView> = unsafe { msg_send![&*self.scroll, animator] };
        animator.setMagnification_centeredAtPoint(zoom, centre);
    }

    /// The canvas point in the middle of what is on screen, where added files go.
    pub fn visible_centre(&self) -> (f64, f64) {
        let visible = self.scroll.contentView().bounds();
        self.document.to_canvas(NSPoint::new(
            visible.origin.x + visible.size.width / 2.,
            visible.origin.y + visible.size.height / 2.,
        ))
    }

    /// Gives the canvas the keyboard.
    pub fn focus(&self) {
        if let Some(window) = self.document.window() {
            window.makeFirstResponder(Some(&self.document));
        }
    }

    /// Whether the canvas has the keyboard.
    pub fn is_focused(&self) -> bool {
        self.document.window().is_some_and(|window| {
            window.firstResponder().is_some_and(|responder| {
                std::ptr::eq(&*responder as *const _ as *const Document, &*self.document)
            })
        })
    }
}
