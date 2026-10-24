//! Icon switch on a Liquid Glass capsule, like the tool pickers in macOS 26 toolbars.
//!
//! AppKit only gives segmented controls that look (a glass track, a soft selection pill and
//! a glass lens that lifts while you press and drag) inside an NSToolbar. Outside one, this
//! view rebuilds it from public pieces: an NSGlassEffectView track, an NSBox pill and a clear
//! NSGlassEffectView lens animated with Core Animation.

use std::cell::{Cell, OnceCell};

use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSBox, NSBoxType, NSColor, NSEvent,
    NSGlassEffectView, NSGlassEffectViewStyle, NSImage, NSImageSymbolConfiguration, NSImageView,
    NSTitlePosition, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_quartz_core::CAMediaTimingFunction;

const PAD: f64 = 2.;
const SLOT: f64 = 40.;
/// How far the lens grows past an item while pressed.
const LENS_GROW: NSSize = NSSize::new(14., 10.);
const ICON_SIZE: f64 = 15.;

pub(crate) struct SwitchIvars {
    height: f64,
    selected: Cell<usize>,
    pressed: Cell<bool>,
    content: OnceCell<Retained<NSView>>,
    pill: OnceCell<Retained<NSBox>>,
    lens: OnceCell<Retained<NSGlassEffectView>>,
    icons: OnceCell<Vec<Retained<NSImageView>>>,
    on_change: OnceCell<Box<dyn Fn(usize)>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryGlassSwitch"]
    #[ivars = SwitchIvars]
    pub(crate) struct GlassSwitch;

    impl GlassSwitch {
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> Bool {
            Bool::YES
        }

        // Every click inside lands here rather than on the icons or the glass.
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            hit.map(|_| self.retain().into_super())
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let index = self.index_at(self.local_x(event));
            self.ivars().pressed.set(true);
            let lens = self.lens();
            lens.setFrame(self.item_rect(index));
            lens.setAlphaValue(0.);
            animate(0.35, true, || {
                lens.animator().setFrame(self.lens_rect(self.item_center(index)));
                lens.animator().setAlphaValue(1.);
                self.pill().animator().setAlphaValue(0.);
            });
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            if !self.ivars().pressed.get() {
                return;
            }
            let first = self.item_center(0);
            let last = self.item_center(self.count() - 1);
            let x = self.local_x(event).clamp(first, last);
            animate(0.12, false, || {
                self.lens().animator().setFrame(self.lens_rect(x));
            });
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {
            if !self.ivars().pressed.replace(false) {
                return;
            }
            let lens = self.lens();
            let frame = lens.frame();
            let index = self.index_at(frame.origin.x + frame.size.width / 2.);
            self.select_animated(index);
            animate(0.35, true, || {
                lens.animator().setFrame(self.item_rect(index));
                lens.animator().setAlphaValue(0.);
            });
            if let Some(on_change) = self.ivars().on_change.get() {
                on_change(index);
            }
        }
    }
);

impl GlassSwitch {
    /// A switch with one item per `(symbol, description)`, `height` points tall.
    pub(crate) fn new(
        mtm: MainThreadMarker,
        icons: &[(&str, &str)],
        height: f64,
        on_change: impl Fn(usize) + 'static,
    ) -> Option<Retained<Self>> {
        // NSGlassEffectView exists from macOS 26 on.
        objc2::runtime::AnyClass::get(c"NSGlassEffectView")?;
        let width = PAD * 2. + SLOT * icons.len() as f64;
        let bounds = rect(0., 0., width, height);
        let this = Self::alloc(mtm).set_ivars(SwitchIvars {
            height,
            selected: Cell::new(0),
            pressed: Cell::new(false),
            content: OnceCell::new(),
            pill: OnceCell::new(),
            lens: OnceCell::new(),
            icons: OnceCell::new(),
            on_change: OnceCell::new(),
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: bounds] };
        let _ = this.ivars().on_change.set(Box::new(on_change));

        let track = NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), bounds);
        track.setCornerRadius(height / 2.);
        let content = NSView::initWithFrame(NSView::alloc(mtm), bounds);
        track.setContentView(Some(&content));
        this.addSubview(&track);

        let pill = NSBox::initWithFrame(NSBox::alloc(mtm), this.item_rect(0));
        pill.setBoxType(NSBoxType::Custom);
        pill.setTitlePosition(NSTitlePosition::NoTitle);
        pill.setBorderWidth(0.);
        pill.setCornerRadius(this.item_height() / 2.);
        pill.setFillColor(&NSColor::labelColor().colorWithAlphaComponent(0.1));
        content.addSubview(&pill);

        let config = NSImageSymbolConfiguration::configurationWithPointSize_weight(ICON_SIZE, 0.);
        let mut views = Vec::new();
        for (i, (symbol, description)) in icons.iter().enumerate() {
            let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                &NSString::from_str(symbol),
                Some(&NSString::from_str(description)),
            )?
            .imageWithSymbolConfiguration(&config)?;
            let view = NSImageView::imageViewWithImage(&image, mtm);
            view.setFrame(this.item_rect(i));
            content.addSubview(&view);
            views.push(view);
        }

        let lens =
            NSGlassEffectView::initWithFrame(NSGlassEffectView::alloc(mtm), this.item_rect(0));
        lens.setStyle(NSGlassEffectViewStyle::Clear);
        lens.setCornerRadius((this.item_height() + LENS_GROW.height) / 2.);
        lens.setAlphaValue(0.);
        this.addSubview(&lens);

        let _ = this.ivars().content.set(content);
        let _ = this.ivars().pill.set(pill);
        let _ = this.ivars().lens.set(lens);
        let _ = this.ivars().icons.set(views);
        this.tint_icons();
        Some(this)
    }

    /// Selects `index` without reporting it back.
    pub(crate) fn select(&self, index: usize) {
        if index != self.ivars().selected.get() && index < self.count() {
            self.select_animated(index);
        }
    }

    fn select_animated(&self, index: usize) {
        self.ivars().selected.set(index);
        self.tint_icons();
        animate(0.35, true, || {
            let pill = self.pill().animator();
            pill.setFrame(self.item_rect(index));
            pill.setAlphaValue(1.);
        });
    }

    fn tint_icons(&self) {
        let selected = self.ivars().selected.get();
        for (i, icon) in self.ivars().icons.get().into_iter().flatten().enumerate() {
            let color = if i == selected {
                NSColor::labelColor()
            } else {
                NSColor::secondaryLabelColor()
            };
            icon.setContentTintColor(Some(&color));
        }
    }

    fn count(&self) -> usize {
        self.ivars().icons.get().map_or(1, Vec::len).max(1)
    }

    fn pill(&self) -> &NSBox {
        self.ivars().pill.get().unwrap()
    }

    fn lens(&self) -> &NSGlassEffectView {
        self.ivars().lens.get().unwrap()
    }

    fn local_x(&self, event: &NSEvent) -> f64 {
        self.convertPoint_fromView(event.locationInWindow(), None).x
    }

    fn index_at(&self, x: f64) -> usize {
        (((x - PAD) / SLOT).floor().max(0.) as usize).min(self.count() - 1)
    }

    fn item_height(&self) -> f64 {
        self.ivars().height - PAD * 2.
    }

    fn item_center(&self, index: usize) -> f64 {
        PAD + SLOT * (index as f64 + 0.5)
    }

    fn item_rect(&self, index: usize) -> NSRect {
        rect(PAD + SLOT * index as f64, PAD, SLOT, self.item_height())
    }

    fn lens_rect(&self, center_x: f64) -> NSRect {
        let width = SLOT + LENS_GROW.width;
        let height = self.item_height() + LENS_GROW.height;
        rect(
            center_x - width / 2.,
            (self.ivars().height - height) / 2.,
            width,
            height,
        )
    }
}

/// Runs `changes` in an animation group; `springy` overshoots a little like iOS.
fn animate(duration: f64, springy: bool, changes: impl FnOnce()) {
    NSAnimationContext::beginGrouping();
    let context = NSAnimationContext::currentContext();
    context.setDuration(duration);
    context.setAllowsImplicitAnimation(true);
    let timing = if springy {
        CAMediaTimingFunction::functionWithControlPoints(0.34, 1.4, 0.64, 1.)
    } else {
        CAMediaTimingFunction::functionWithControlPoints(0.25, 0.1, 0.25, 1.)
    };
    context.setTimingFunction(Some(&timing));
    changes();
    NSAnimationContext::endGrouping();
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
}
