//! The popover under the box's icon, laid out like the one under a shortcut's icon in
//! Shortcuts without the icon row, which the toolbar already shows: the color dots, a line,
//! then a symbol search over the glyphs in named sections.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, msg_send, sel};
use objc2_app_kit::{
    NSBorderType, NSBox, NSBoxType, NSButton, NSColor, NSControlSize, NSFont, NSImageScaling,
    NSImageView, NSPopover, NSPopoverBehavior, NSScrollView, NSSearchField, NSTextField, NSView,
    NSViewController,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

use crate::native::Target;
use crate::scroller::FlippedView;
use crate::toolbar::{ToolbarEvent, box_color, glyph_image, place_symbol, sized_symbol};

const WIDTH: f64 = 310.;
/// Color dots: Shortcuts' are 17 points, 29 apart, ten to a row.
const DOT: f64 = 17.;
const DOT_PITCH: f64 = 29.;
const DOT_COLUMNS: usize = 10;
const SEARCH_HEIGHT: f64 = 28.;
/// Height of the scrolling list of glyphs.
const GLYPHS_HEIGHT: f64 = 300.;
const GLYPH_COLUMNS: usize = 8;
const GLYPH_ROW: f64 = 42.;
const GLYPH_SIZE: f64 = 18.;
const GLYPH_HIGHLIGHT: f64 = 34.;
const SECTION_HEADER: f64 = 30.;
const SIDE: f64 = 16.;
/// Tags of the glyph buttons start here; colors use 0 up.
const GLYPH_TAG: isize = 1000;

pub(crate) struct IconPicker {
    pub(crate) popover: Retained<NSPopover>,
    checks: Vec<(String, Retained<NSImageView>)>,
    groups: Vec<(String, Vec<String>)>,
    document: Retained<FlippedView>,
    scroll: Retained<NSScrollView>,
    /// The glyph buttons now shown, by symbol name; a button's tag is `GLYPH_TAG` plus its
    /// place here.
    glyphs: RefCell<Vec<(String, Retained<NSButton>)>>,
    /// The box's color and glyph.
    current: RefCell<(String, String)>,
    target: Retained<Target>,
}

impl IconPicker {
    pub(crate) fn new(
        mtm: MainThreadMarker,
        colors: &[&str],
        groups: &[(&str, &[&str])],
        on_event: Rc<dyn Fn(ToolbarEvent)>,
    ) -> Rc<Self> {
        Rc::new_cyclic(|this: &Weak<Self>| {
            let target = Target::new(mtm, {
                let this = this.clone();
                move |sender| {
                    let Some(this) = this.upgrade() else {
                        return;
                    };
                    if let Some(field) = sender.downcast_ref::<NSSearchField>() {
                        this.show_glyphs(&field.stringValue().to_string());
                    } else if let Some(button) = sender.downcast_ref::<NSButton>() {
                        let tag = button.tag();
                        if tag >= GLYPH_TAG {
                            let name = this
                                .glyphs
                                .borrow()
                                .get((tag - GLYPH_TAG) as usize)
                                .map(|(name, _)| name.clone());
                            if let Some(name) = name {
                                on_event(ToolbarEvent::Symbol(name));
                            }
                        } else {
                            on_event(ToolbarEvent::Color(tag.max(0) as usize));
                        }
                    }
                }
            });

            let dot_rows = colors.len().div_ceil(DOT_COLUMNS);
            let dots_height = (dot_rows - 1) as f64 * DOT_PITCH + DOT;
            let dots_section = 16. + dots_height + 16.;
            let search_section = 12. + SEARCH_HEIGHT + 8.;
            let height = dots_section + 1. + search_section + GLYPHS_HEIGHT;
            let view = NSView::initWithFrame(
                NSView::alloc(mtm),
                NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, height)),
            );
            // AppKit counts y from the bottom; `y` walks down from the top.
            let mut y = height;

            y -= 16.;
            let dots_left = (WIDTH - (DOT_COLUMNS - 1) as f64 * DOT_PITCH - DOT) / 2.;
            let mut checks = Vec::new();
            for (index, color) in colors.iter().enumerate() {
                let row = index / DOT_COLUMNS;
                let column = index % DOT_COLUMNS;
                let button = plain_button(mtm, &target, index as isize, color);
                if let Some(layer) = button.layer() {
                    layer.setCornerRadius(DOT / 2.);
                    layer.setBackgroundColor(Some(&box_color(color).CGColor()));
                }
                button.setFrame(NSRect::new(
                    NSPoint::new(
                        dots_left + column as f64 * DOT_PITCH,
                        y - DOT - row as f64 * DOT_PITCH,
                    ),
                    NSSize::new(DOT, DOT),
                ));
                let check = NSImageView::initWithFrame(NSImageView::alloc(mtm), NSRect::ZERO);
                check.setImageScaling(NSImageScaling::ScaleNone);
                check.setContentTintColor(Some(&NSColor::whiteColor()));
                place_symbol(
                    &check,
                    sized_symbol("checkmark", "Chosen", 9.).as_deref(),
                    DOT,
                );
                check.setHidden(true);
                button.addSubview(&check);
                view.addSubview(&button);
                checks.push((color.to_string(), check));
            }
            y -= dots_height + 16.;

            y -= 1.;
            view.addSubview(&separator(mtm, y));

            y -= 12. + SEARCH_HEIGHT;
            let search = NSSearchField::initWithFrame(
                NSSearchField::alloc(mtm),
                NSRect::new(
                    NSPoint::new(SIDE, y),
                    NSSize::new(WIDTH - SIDE * 2., SEARCH_HEIGHT),
                ),
            );
            search.setControlSize(NSControlSize::Large);
            search.setPlaceholderString(Some(&NSString::from_str("Search Symbols")));
            search.setSendsSearchStringImmediately(true);
            unsafe {
                search.setTarget(Some(&target));
                search.setAction(Some(sel!(fire:)));
            }
            view.addSubview(&search);
            y -= 8.;

            let scroll = NSScrollView::initWithFrame(
                NSScrollView::alloc(mtm),
                NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, y)),
            );
            scroll.setDrawsBackground(false);
            scroll.setBorderType(NSBorderType::NoBorder);
            scroll.setHasVerticalScroller(true);
            scroll.setAutohidesScrollers(true);
            let document: Retained<FlippedView> = unsafe {
                msg_send![
                    FlippedView::alloc(mtm),
                    initWithFrame: NSRect::new(NSPoint::ZERO, NSSize::new(WIDTH, y))
                ]
            };
            scroll.setDocumentView(Some(&document));
            view.addSubview(&scroll);

            let popover = NSPopover::new(mtm);
            popover.setBehavior(NSPopoverBehavior::Transient);
            let controller =
                NSViewController::initWithNibName_bundle(NSViewController::alloc(mtm), None, None);
            controller.setView(&view);
            popover.setContentViewController(Some(&controller));
            popover.setContentSize(NSSize::new(WIDTH, height));

            let picker = Self {
                popover,
                checks,
                groups: groups
                    .iter()
                    .map(|(name, symbols)| {
                        (
                            name.to_string(),
                            symbols.iter().map(|symbol| symbol.to_string()).collect(),
                        )
                    })
                    .collect(),
                document,
                scroll,
                glyphs: RefCell::new(Vec::new()),
                current: RefCell::new((String::new(), String::new())),
                target,
            };
            picker.show_glyphs("");
            picker
        })
    }

    /// Shows the sections with glyphs whose name or section matches `query`.
    fn show_glyphs(&self, query: &str) {
        let mtm = MainThreadMarker::from(&*self.target);
        let query = query.trim().to_lowercase();
        for view in self.document.subviews().iter() {
            view.removeFromSuperview();
        }
        let mut glyphs = Vec::new();
        let cell = (WIDTH - 12. * 2.) / GLYPH_COLUMNS as f64;
        // The document is flipped: `y` walks down from its top.
        let mut y = 0.;
        for (section, symbols) in &self.groups {
            let section_matches = section.to_lowercase().contains(&query);
            let shown: Vec<&String> = symbols
                .iter()
                .filter(|symbol| {
                    query.is_empty() || section_matches || symbol.replace('.', " ").contains(&query)
                })
                .filter(|symbol| glyph_image(symbol, GLYPH_SIZE).is_some())
                .collect();
            if shown.is_empty() {
                continue;
            }
            let header =
                NSTextField::labelWithString(&NSString::from_str(&section.to_uppercase()), mtm);
            header.setFont(Some(&NSFont::systemFontOfSize_weight(12., unsafe {
                objc2_app_kit::NSFontWeightMedium
            })));
            header.setTextColor(Some(&NSColor::secondaryLabelColor()));
            header.sizeToFit();
            let header_height = header.frame().size.height;
            header.setFrameOrigin(NSPoint::new(
                SIDE + 4.,
                y + (SECTION_HEADER - header_height) / 2.,
            ));
            self.document.addSubview(&header);
            y += SECTION_HEADER;

            let rows = shown.len().div_ceil(GLYPH_COLUMNS);
            let band = NSView::initWithFrame(
                NSView::alloc(mtm),
                NSRect::new(
                    NSPoint::new(0., y),
                    NSSize::new(WIDTH, rows as f64 * GLYPH_ROW + 8.),
                ),
            );
            band.setWantsLayer(true);
            if let Some(layer) = band.layer() {
                let shade = NSColor::windowBackgroundColor().colorWithAlphaComponent(0.45);
                layer.setBackgroundColor(Some(&shade.CGColor()));
            }
            self.document.addSubview(&band);
            y += 4.;
            for (index, symbol) in shown.iter().enumerate() {
                let row = index / GLYPH_COLUMNS;
                let column = index % GLYPH_COLUMNS;
                let tag = GLYPH_TAG + glyphs.len() as isize;
                let button = plain_button(mtm, &self.target, tag, symbol);
                button.setImage(glyph_image(symbol, GLYPH_SIZE).as_deref());
                button.setImageScaling(NSImageScaling::ScaleNone);
                button.setContentTintColor(Some(&NSColor::labelColor()));
                if let Some(layer) = button.layer() {
                    layer.setCornerRadius(8.);
                }
                button.setFrame(NSRect::new(
                    NSPoint::new(
                        12. + column as f64 * cell + (cell - GLYPH_HIGHLIGHT) / 2.,
                        y + row as f64 * GLYPH_ROW + (GLYPH_ROW - GLYPH_HIGHLIGHT) / 2.,
                    ),
                    NSSize::new(GLYPH_HIGHLIGHT, GLYPH_HIGHLIGHT),
                ));
                self.document.addSubview(&button);
                glyphs.push((symbol.to_string(), button));
            }
            y += rows as f64 * GLYPH_ROW + 4.;
        }
        let visible = self.scroll.contentSize().height;
        self.document
            .setFrameSize(NSSize::new(WIDTH, y.max(visible)));
        self.document.scrollPoint(NSPoint::ZERO);
        *self.glyphs.borrow_mut() = glyphs;
        self.mark_glyph();
    }

    /// Checks the box's color and highlights its glyph.
    pub(crate) fn mark(&self, color: &str, glyph: &str) {
        *self.current.borrow_mut() = (color.to_string(), glyph.to_string());
        for (name, check) in &self.checks {
            check.setHidden(name != color);
        }
        self.mark_glyph();
    }

    fn mark_glyph(&self) {
        let glyph = self.current.borrow().1.clone();
        let highlight = NSColor::quaternaryLabelColor().CGColor();
        for (name, button) in self.glyphs.borrow().iter() {
            if let Some(layer) = button.layer() {
                layer.setBackgroundColor((*name == glyph).then_some(&*highlight));
            }
        }
    }
}

fn separator(mtm: MainThreadMarker, y: f64) -> Retained<NSBox> {
    let line = NSBox::initWithFrame(
        NSBox::alloc(mtm),
        NSRect::new(NSPoint::new(0., y), NSSize::new(WIDTH, 1.)),
    );
    line.setBoxType(NSBoxType::Separator);
    line
}

/// A borderless, layer-backed button reporting to `target`.
fn plain_button(
    mtm: MainThreadMarker,
    target: &Target,
    tag: isize,
    tooltip: &str,
) -> Retained<NSButton> {
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &NSString::new(),
            Some(target),
            Some(sel!(fire:)),
            mtm,
        )
    };
    button.setBordered(false);
    button.setTag(tag);
    button.setToolTip(Some(&NSString::from_str(tooltip)));
    button.setWantsLayer(true);
    button
}
