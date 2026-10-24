//! A real NSScrollView over a canvas GPUI draws, there only for its scroll bars: the same
//! overlay scrollers Freeform shows, appearing while the canvas moves and fading out after.
//! Clicks and wheel events outside the scrollers pass through to the canvas underneath.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBorderType, NSResponder, NSScrollView, NSScrollerStyle, NSView,
    NSViewBoundsDidChangeNotification,
};
use objc2_foundation::{NSNotificationCenter, NSPoint, NSRect, NSSize};

use crate::native::{NativeView, Target};

define_class!(
    // A scroll view that only answers clicks on its scroll bars.
    #[unsafe(super(NSScrollView, NSView, NSResponder, objc2::runtime::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryCanvasScrollView"]
    struct PassThroughScrollView;

    impl PassThroughScrollView {
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            let hit: *mut NSView = unsafe { msg_send![super(self), hitTest: point] };
            let Some(view) = (unsafe { hit.as_ref() }) else {
                return std::ptr::null_mut();
            };
            let on_scroller = [self.verticalScroller(), self.horizontalScroller()]
                .into_iter()
                .flatten()
                .any(|scroller| view.isDescendantOf(&scroller));
            if on_scroller { hit } else { std::ptr::null_mut() }
        }
    }
);

define_class!(
    // The empty document of the scroll view, laid out from the top-left like the canvas.
    #[unsafe(super(NSView, NSResponder, objc2::runtime::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AmbryCanvasDocument"]
    pub(crate) struct FlippedView;

    impl FlippedView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }
    }
);

/// The scroll bars along the right and bottom edges of the canvas.
pub struct CanvasScrollView {
    native: NativeView,
    scroll: Retained<PassThroughScrollView>,
    document: Retained<FlippedView>,
    /// Set while the canvas moves the scroll view, so that move is not reported back.
    syncing: Rc<Cell<bool>>,
    observer: Retained<Target>,
}

impl CanvasScrollView {
    /// `on_scroll` hears the top-left of the visible part of the document, in points, when the
    /// user drags a knob or clicks a track.
    ///
    /// # Safety
    /// `ns_view` must point to a live `NSView`, and this must run on the main thread.
    pub unsafe fn new(
        ns_view: NonNull<c_void>,
        on_scroll: impl Fn(f64, f64) + 'static,
    ) -> Option<Self> {
        let mtm = MainThreadMarker::new()?;
        let parent: &NSView = unsafe { ns_view.cast().as_ref() };
        let frame = NSRect::new(NSPoint::ZERO, NSSize::new(100., 100.));
        let scroll: Retained<PassThroughScrollView> =
            unsafe { msg_send![PassThroughScrollView::alloc(mtm), initWithFrame: frame] };
        scroll.setDrawsBackground(false);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setHasVerticalScroller(true);
        scroll.setHasHorizontalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setScrollerStyle(NSScrollerStyle::Overlay);
        scroll.setAutomaticallyAdjustsContentInsets(false);
        let clip = scroll.contentView();
        clip.setDrawsBackground(false);
        let document: Retained<FlippedView> =
            unsafe { msg_send![FlippedView::alloc(mtm), initWithFrame: frame] };
        scroll.setDocumentView(Some(&document));

        let syncing = Rc::new(Cell::new(false));
        let observer = Target::new(mtm, {
            let syncing = syncing.clone();
            let clip = clip.clone();
            move |_| {
                if !syncing.get() {
                    let origin = clip.bounds().origin;
                    on_scroll(origin.x, origin.y);
                }
            }
        });
        clip.setPostsBoundsChangedNotifications(true);
        unsafe {
            NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
                &observer,
                sel!(fire:),
                Some(NSViewBoundsDidChangeNotification),
                Some(&clip),
            );
        }
        let view: Retained<NSView> = Retained::into_super(Retained::into_super(scroll.clone()));
        Some(Self {
            native: NativeView::host(parent, view, None),
            scroll,
            document,
            syncing,
            observer,
        })
    }

    pub fn native(&self) -> &NativeView {
        &self.native
    }

    /// Makes the document `width` by `height` points and shows it from `(x, y)`.
    pub fn set(&self, width: f64, height: f64, x: f64, y: f64) {
        self.syncing.set(true);
        let size = NSSize::new(width, height);
        if self.document.frame().size != size {
            self.document.setFrameSize(size);
        }
        let clip = self.scroll.contentView();
        let origin = NSPoint::new(x, y);
        if clip.bounds().origin != origin {
            clip.scrollToPoint(origin);
            self.scroll.reflectScrolledClipView(&clip);
        }
        self.syncing.set(false);
    }

    /// Shows the scroll bars for a moment, as when the canvas moves.
    pub fn flash(&self) {
        self.scroll.flashScrollers();
    }
}

impl Drop for CanvasScrollView {
    fn drop(&mut self) {
        let observer: &AnyObject = &self.observer;
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(observer) };
    }
}
